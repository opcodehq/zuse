//! Browser-compatible GPUI views. Native process/filesystem access stays behind RPC.

pub mod input;

use futures::{
    StreamExt,
    channel::mpsc::{UnboundedReceiver, UnboundedSender},
};
use gpui::{prelude::*, *};
use serde_json::{Value, json};
use std::collections::HashMap;
use zuse_model::{SessionMessageEntry, TranscriptUpdate, apply_transcript_frame};
use zuse_protocol::methods;

actions!(zuse_client, [Submit]);

pub struct Transport {
    pub requests: UnboundedSender<Value>,
    pub events: UnboundedReceiver<Value>,
}

pub struct WorkspaceView {
    input: Entity<input::TextInput>,
    requests: UnboundedSender<Value>,
    next_id: u64,
    pending: HashMap<u64, String>,
    chats: Vec<Value>,
    providers: Vec<Value>,
    selected: Option<Value>,
    messages: Vec<SessionMessageEntry>,
    transcript_watch: Option<u64>,
    device_id: Option<String>,
    status: String,
    error: Option<String>,
    show_archive: bool,
    settings: bool,
    provider_index: usize,
    send_draft: HashMap<u64, String>,
    creating: Option<(u64, Value, Value)>,
    _reader: Task<()>,
}

impl WorkspaceView {
    pub fn new(transport: Transport, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| input::TextInput::new("Ask Zuse to work on something…", cx));
        let mut events = transport.events;
        let reader = cx.spawn(async move |this, cx| {
            while let Some(frame) = events.next().await {
                if this.update(cx, |this, cx| this.receive(frame, cx)).is_err() {
                    break;
                }
            }
            let _ = this.update(cx, |this, cx| {
                this.status = "Disconnected".into();
                cx.notify();
            });
        });
        let mut view = Self {
            input,
            requests: transport.requests,
            next_id: 1,
            pending: HashMap::new(),
            chats: vec![],
            providers: vec![],
            selected: None,
            messages: vec![],
            transcript_watch: None,
            device_id: None,
            status: "Connecting to your engine…".into(),
            error: None,
            show_archive: false,
            settings: false,
            provider_index: 0,
            send_draft: HashMap::new(),
            creating: None,
            _reader: reader,
        };
        view.call(methods::LOCAL_DEVICE, json!({}));
        view.call(methods::LIST_HARNESSES, json!({}));
        view.call(methods::WATCH_CHATS, json!({}));
        view
    }

    fn call(&mut self, method: &str, params: Value) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.pending.insert(id, method.into());
        if self
            .requests
            .unbounded_send(json!({"id":id,"method":method,"params":params}))
            .is_err()
        {
            self.error = Some("The engine connection is closed. Reconnect before sending.".into());
        }
        id
    }

    fn receive(&mut self, frame: Value, cx: &mut Context<Self>) {
        if let Some(connection) = frame.get("connection").and_then(Value::as_str) {
            self.status = connection.into();
            cx.notify();
            return;
        }
        let Some(id) = frame.get("id").and_then(Value::as_u64) else {
            return;
        };
        let method = self.pending.get(&id).cloned().unwrap_or_default();
        if let Some(error) = frame.get("err").and_then(Value::as_str) {
            self.error = Some(error.into());
            if let Some(draft) = self.send_draft.remove(&id) {
                if self.input.read(cx).value().is_empty() {
                    self.input
                        .update(cx, |input, cx| input.set_value(draft, cx));
                }
            }
            self.pending.remove(&id);
            if self
                .creating
                .as_ref()
                .is_some_and(|(request, _, _)| *request == id)
            {
                self.creating = None;
            }
            cx.notify();
            return;
        }
        if frame.get("ok").is_some()
            && self
                .creating
                .as_ref()
                .is_some_and(|(request, _, _)| *request == id)
        {
            let (_, chat, command) = self.creating.take().unwrap();
            self.pending.remove(&id);
            let draft = self.send_draft.remove(&id).unwrap_or_default();
            self.select(chat.clone(), cx);
            let send = self.call(
                methods::QUEUE_COMMAND,
                json!({"chatId":chat["id"],"command":command}),
            );
            self.send_draft.insert(send, draft);
            cx.notify();
            return;
        }
        let value = frame.get("item").or_else(|| frame.get("ok"));
        if let Some(value) = value {
            match method.as_str() {
                methods::LOCAL_DEVICE => {
                    self.device_id = value
                        .get("deviceId")
                        .and_then(Value::as_str)
                        .map(str::to_owned);
                    self.status = "Local engine connected".into();
                }
                methods::LIST_HARNESSES => {
                    self.providers = value
                        .as_array()
                        .cloned()
                        .unwrap_or_default()
                        .into_iter()
                        .filter(|p| p["id"] != "mock")
                        .collect()
                }
                methods::WATCH_CHATS => self.chats = value.as_array().cloned().unwrap_or_default(),
                methods::WATCH_DOC_MESSAGES if self.transcript_watch == Some(id) => {
                    match serde_json::from_value::<TranscriptUpdate>(value.clone()) {
                        Ok(update) => {
                            if let Err(error) =
                                apply_transcript_frame(&mut self.messages, update.frame)
                            {
                                self.error =
                                    Some(format!("Transcript needs resynchronization: {error}"));
                            }
                        }
                        Err(error) => {
                            self.error = Some(format!("Invalid transcript update: {error}"))
                        }
                    }
                }
                methods::QUEUE_COMMAND => {
                    self.send_draft.remove(&id);
                }
                _ => {}
            }
        }
        if frame.get("ok").is_some() || frame["done"] == true {
            self.pending.remove(&id);
        }
        cx.notify();
    }

    fn select(&mut self, chat: Value, cx: &mut Context<Self>) {
        if let Some(id) = self.transcript_watch.take() {
            let _ = self.requests.unbounded_send(json!({"id":id,"cancel":true}));
            self.pending.remove(&id);
        }
        let id = self.call(methods::WATCH_DOC_MESSAGES, json!({"chatId":chat["id"]}));
        self.transcript_watch = Some(id);
        self.messages.clear();
        self.selected = Some(chat);
        self.settings = false;
        self.error = None;
        cx.notify();
    }

    fn new_chat(&mut self, cx: &mut Context<Self>) {
        self.selected = None;
        self.messages.clear();
        self.settings = false;
        self.input
            .update(cx, |input, cx| input.set_value(String::new(), cx));
        cx.notify();
    }

    fn submit(&mut self, _: &Submit, _: &mut Window, cx: &mut Context<Self>) {
        if self.creating.is_some() {
            return;
        }
        let text = self.input.read(cx).value();
        if text.trim().is_empty() {
            return;
        }
        let Some(device) = self.device_id.clone() else {
            self.error = Some("Connect to an engine before sending.".into());
            cx.notify();
            return;
        };
        let Some(provider) = self.providers.get(self.provider_index).cloned() else {
            self.error = Some("Set up an agent in Settings first.".into());
            cx.notify();
            return;
        };
        if provider["installed"] == false {
            self.error = Some(format!(
                "{} is not installed on the engine host.",
                provider["name"].as_str().unwrap_or("This agent")
            ));
            cx.notify();
            return;
        }
        let command = json!({"kind":"run","messageId":uuid::Uuid::new_v4().to_string(),"request":{"prompt":text,"harness":provider["id"],"model":null,"reasoning":null,"cwd":"","sandbox":"workspace-write","resume":null}});
        let send = if let Some(chat) = &self.selected {
            self.call(
                methods::QUEUE_COMMAND,
                json!({"chatId":chat["id"],"command":command}),
            )
        } else {
            // IDs are client-owned, allowing the durable send to refer to the same chat.
            let id = uuid::Uuid::new_v4().to_string();
            let request = self.call(methods::MUTATE, json!({"op":"createChat","chatId":id,"deviceId":device,"config":{"harness":provider["id"],"model":null,"reasoning":null,"sandbox":"workspace-write"}}));
            let chat = json!({"id":id,"title":text.chars().take(70).collect::<String>(),"deviceId":device});
            self.creating = Some((request, chat, command));
            request
        };
        self.send_draft.insert(send, text);
        self.input
            .update(cx, |input, cx| input.set_value(String::new(), cx));
        self.error = None;
        cx.notify();
    }

    fn button(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Stateful<Div> {
        div()
            .id(id)
            .px_3()
            .py_2()
            .rounded_md()
            .cursor_pointer()
            .text_sm()
            .text_color(rgb(0xc6c6cc))
            .hover(|s| s.bg(rgb(0x29292e)))
            .child(label.into())
    }
}

impl Render for WorkspaceView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let chats = self
            .chats
            .iter()
            .filter(|chat| chat["archived"].as_bool().unwrap_or(false) == self.show_archive)
            .cloned()
            .collect::<Vec<_>>();
        let sidebar = div()
            .w(px(244.))
            .h_full()
            .flex_shrink_0()
            .bg(rgb(0x101012))
            .p_3()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .px_3()
                    .py_3()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_lg()
                    .child("zuse"),
            )
            .child(
                Self::button("new-chat", "+ New thread")
                    .on_click(cx.listener(|this, _, _, cx| this.new_chat(cx))),
            )
            .child(
                Self::button(
                    "archive",
                    if self.show_archive {
                        "← Active threads"
                    } else {
                        "Archive"
                    },
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.show_archive = !this.show_archive;
                    cx.notify();
                })),
            )
            .child(
                div()
                    .px_3()
                    .mt_4()
                    .mb_2()
                    .text_xs()
                    .text_color(rgb(0x777780))
                    .child(if self.show_archive {
                        "ARCHIVED"
                    } else {
                        "THREADS"
                    }),
            )
            .child(
                div()
                    .id("threads")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .children(chats.into_iter().map(|chat| {
                        let title = chat["title"]
                            .as_str()
                            .unwrap_or("Untitled thread")
                            .to_owned();
                        let id = chat["id"].as_str().unwrap_or_default().to_owned();
                        Self::button(SharedString::from(id), SharedString::from(title)).on_click(
                            cx.listener(move |this, _, _, cx| this.select(chat.clone(), cx)),
                        )
                    })),
            )
            .child(
                Self::button("settings", "Settings").on_click(cx.listener(|this, _, _, cx| {
                    this.settings = !this.settings;
                    cx.notify();
                })),
            )
            .child(
                div()
                    .px_3()
                    .py_2()
                    .text_xs()
                    .text_color(rgb(0x777780))
                    .child(self.status.clone()),
            );

        let title = self
            .selected
            .as_ref()
            .and_then(|c| c["title"].as_str())
            .unwrap_or("New thread")
            .to_owned();
        let mut center = div().flex_1().min_w_0().h_full().flex().flex_col().child(
            div()
                .h(px(54.))
                .px_6()
                .flex()
                .items_center()
                .border_b_1()
                .border_color(rgb(0x26262b))
                .text_sm()
                .child(title),
        );
        if self.settings {
            center = center.child(div().id("settings-content").flex_1().overflow_y_scroll().p_8().flex().flex_col().gap_4()
                .child(div().text_xl().child("Agent setup"))
                .child(div().text_sm().text_color(rgb(0x96969f)).child("Agents run on the connected engine host. Installed CLI credentials stay on that machine."))
                .children(self.providers.iter().enumerate().map(|(index,p)| {
                    Self::button(("provider",index), SharedString::from(format!("{}  ·  {}",p["name"].as_str().unwrap_or("Agent"), if p["installed"] == true { "Installed" } else { "Not installed" })))
                        .on_click(cx.listener(move |this, _, _, cx| { this.provider_index = index; this.settings = false; cx.notify(); }))
                })));
        } else {
            let timeline = div()
                .id("transcript")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .px_8()
                .py_6()
                .flex()
                .flex_col()
                .gap_5()
                .when(self.messages.is_empty(), |d| {
                    d.child(
                        div()
                            .flex_1()
                            .flex()
                            .flex_col()
                            .justify_center()
                            .items_center()
                            .gap_3()
                            .child(
                                div()
                                    .text_2xl()
                                    .font_weight(FontWeight::MEDIUM)
                                    .child("What are we building?"),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(rgb(0x8a8a94))
                                    .child("Your workspace. Your agents. One place to work."),
                            ),
                    )
                })
                .children(self.messages.iter().enumerate().map(|(index, message)| {
                    let message = serde_json::to_value(message).unwrap_or(Value::Null);
                    let parts = message
                        .get("parts")
                        .and_then(Value::as_array)
                        .cloned()
                        .unwrap_or_default();
                    div()
                        .id(("message", index))
                        .w_full()
                        .max_w(px(800.))
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child(
                            div()
                                .text_xs()
                                .text_color(rgb(0x888892))
                                .child(message["role"].as_str().unwrap_or("assistant").to_owned()),
                        )
                        .children(parts.into_iter().filter_map(|part| {
                            part.get("text")
                                .or_else(|| part.get("message"))
                                .and_then(Value::as_str)
                                .map(|s| div().text_sm().child(s.to_owned()))
                        }))
                }));
            center = center
                .child(timeline)
                .when_some(self.error.clone(), |d, error| {
                    d.child(
                        div()
                            .mx_6()
                            .mb_2()
                            .p_3()
                            .rounded_md()
                            .bg(rgb(0x392326))
                            .text_sm()
                            .text_color(rgb(0xf4a0a0))
                            .child(error),
                    )
                })
                .child(
                    div()
                        .mx_6()
                        .mb_5()
                        .p_3()
                        .rounded_xl()
                        .bg(rgb(0x202024))
                        .border_1()
                        .border_color(rgb(0x34343b))
                        .flex()
                        .flex_col()
                        .gap_3()
                        .child(self.input.clone())
                        .child(
                            div()
                                .flex()
                                .justify_between()
                                .items_center()
                                .child(
                                    Self::button(
                                        "provider-picker",
                                        SharedString::from(
                                            self.providers
                                                .get(self.provider_index)
                                                .and_then(|p| p["name"].as_str())
                                                .unwrap_or("Choose an agent")
                                                .to_owned(),
                                        ),
                                    )
                                    .on_click(cx.listener(
                                        |this, _, _, cx| {
                                            this.settings = true;
                                            cx.notify();
                                        },
                                    )),
                                )
                                .child(
                                    Self::button("send", "Send ↑")
                                        .bg(rgb(0xe3e3e8))
                                        .text_color(rgb(0x171719))
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.submit(&Submit, window, cx)
                                        })),
                                ),
                        ),
                );
        }
        div()
            .key_context("Workspace")
            .on_action(cx.listener(Self::submit))
            .size_full()
            .flex()
            .bg(rgb(0x171719))
            .text_color(rgb(0xe5e5ea))
            .font_family("Geist")
            .child(sidebar)
            .child(center)
            .child(
                div()
                    .w(px(260.))
                    .flex_shrink_0()
                    .h_full()
                    .border_l_1()
                    .border_color(rgb(0x26262b))
                    .p_5()
                    .flex()
                    .flex_col()
                    .gap_4()
                    .child(div().text_sm().child("Workspace"))
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(0x85858e))
                            .child("Files · Changes · Terminal"),
                    )
                    .child(
                        div()
                            .mt_3()
                            .text_sm()
                            .text_color(rgb(0x777780))
                            .child("Select a project thread to inspect its workspace."),
                    ),
            )
    }
}

pub fn initialize(cx: &mut App) {
    input::bind_keys(cx);
    cx.bind_keys([KeyBinding::new("enter", Submit, Some("Workspace"))]);
    cx.text_system()
        .add_fonts(vec![std::borrow::Cow::Borrowed(include_bytes!(
            "../../ui/assets/fonts/Geist.ttf"
        ))])
        .expect("bundled UI font");
}
