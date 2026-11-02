//! Shared ACP command dialog; the connected host owns persistence and revisions.
use crate::localization::message as tr;
use crate::{
    input::{self, TextInput},
    palette::rgb,
    settings_style as s,
    shell::{MUTED, TEXT},
};
use gpui::{prelude::*, *};
use zuse_i18n::messages as msg;
use zuse_protocol::custom_acp::{CustomAcpAgent, command};
fn popup_bg() -> Hsla {
    crate::palette::tone(hsla(132. / 360., 0.05, 0.105, 1.), white())
}

#[derive(Clone)]
pub enum Event {
    Save(CustomAcpAgent),
    Dismiss,
}
pub struct CommandDialog {
    original: Option<CustomAcpAgent>,
    existing: bool,
    fields: [Entity<TextInput>; 6],
    _submit: Vec<Subscription>,
    clear_environment: bool,
    advanced: bool,
    busy: bool,
    error: Option<String>,
    root: FocusHandle,
    cancel: FocusHandle,
    save: FocusHandle,
    window: AnyWindowHandle,
    return_focus: Option<FocusHandle>,
}
impl EventEmitter<Event> for CommandDialog {}
impl CommandDialog {
    pub fn new(
        original: Option<CustomAcpAgent>,
        existing: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        input::bind_keys(cx);
        let values = original
            .as_ref()
            .map(|agent| {
                [
                    String::from(agent.id),
                    agent.name.clone(),
                    agent.command.clone(),
                    command::arguments(&agent.args),
                    String::new(),
                    agent.model_ids.join("\n"),
                ]
            })
            .unwrap_or_else(|| {
                [
                    format!("acp-{}", uuid::Uuid::new_v4().simple()),
                    String::new(),
                    String::new(),
                    String::new(),
                    String::new(),
                    String::new(),
                ]
            });
        let labels = [
            "Provider ID (acp-name)",
            "My agent",
            "my-agent",
            "--acp",
            "API_KEY=… OTHER=…",
            "Fallback model IDs — one per line",
        ];
        let fields = std::array::from_fn(|index| {
            cx.new(|cx| {
                let mut field = match index {
                    4 => TextInput::password(labels[index], window, cx),
                    5 => TextInput::multiline(labels[index], window, cx),
                    _ => TextInput::new(labels[index], window, cx),
                };
                field.set_value(values[index].clone(), cx);
                field
            })
        });
        let mut submit: Vec<Subscription> = fields
            .iter()
            .map(|field| cx.subscribe(field, |this, _, _: &input::Submitted, cx| this.submit(cx)))
            .collect();
        submit.extend(fields.iter().map(|field| {
            // Typing repaints the field; only focus changes its surrounding border.
            cx.subscribe(field, |_, _, _: &input::FocusChanged, cx| cx.notify())
        }));
        let return_focus = window.focused(cx);
        let first = fields[1].read(cx).focus_handle(cx);
        window.on_next_frame(move |window, cx| window.focus(&first, cx));
        Self {
            original,
            existing,
            fields,
            _submit: submit,
            clear_environment: false,
            advanced: false,
            busy: false,
            error: None,
            root: cx.focus_handle(),
            cancel: cx.focus_handle().tab_stop(true),
            save: cx.focus_handle().tab_stop(true),
            window: window.window_handle(),
            return_focus,
        }
    }
    pub fn set_status(&mut self, busy: bool, error: Option<String>, cx: &mut Context<Self>) {
        self.busy = busy;
        self.error = error;
        cx.notify();
    }
    pub fn restore_focus(&self, cx: &mut Context<Self>) {
        if let Some(focus) = self.return_focus.clone() {
            let handle = self.window;
            cx.defer(move |cx| {
                let _ = cx.update_window(handle, move |_, window, cx| window.focus(&focus, cx));
            });
        }
    }
    pub fn set_return_focus(&mut self, focus: Option<FocusHandle>) {
        self.return_focus = focus;
    }
    pub fn dismiss(&mut self, cx: &mut Context<Self>) {
        self.restore_focus(cx);
        cx.emit(Event::Dismiss);
    }
    fn submit(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let fields = std::array::from_fn(|index| self.fields[index].read(cx).value(cx));
        let preserved = (!self.clear_environment)
            .then(|| self.original.as_ref().map(|a| &a.env))
            .flatten();
        match command::definition(&fields, preserved) {
            Ok(agent) => {
                self.busy = true;
                self.error = None;
                cx.emit(Event::Save(agent));
            }
            Err(error) => self.error = Some(error),
        }
        cx.notify();
    }
    fn field(&self, index: usize, label: &str, window: &Window, cx: &App) -> Div {
        let focus = self.fields[index].read(cx).focus_handle(cx);
        let focused = focus.is_focused(window);
        let border = if crate::palette::is_light() { 0. } else { 2. };
        let width = (f32::from(window.viewport_size().width) - 64. - border).min(352. - border);
        div()
            .w(px(width))
            .flex_none()
            .flex()
            .flex_col()
            .gap(px(4.))
            .child(
                div()
                    .h(px(14.85))
                    .flex_none()
                    .text_size(px(11.))
                    .line_height(px(14.85))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(rgb(MUTED))
                    .child(label.to_owned()),
            )
            .child(
                div()
                    .id(("acp-command-field", index))
                    .w(px(width))
                    .flex_none()
                    .h(px(if index == 5 { 60. } else { 28. }))
                    .cursor_text()
                    .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                        window.focus(&focus, cx)
                    })
                    .px(px(9.))
                    .rounded(px(6.))
                    .border_1()
                    .border_color(if focused {
                        crate::palette::tone(
                            rgba(0xd9c5108c).into(),
                            hsla(83. / 360., 0.74, 0.43, 1.),
                        )
                    } else {
                        crate::palette::tone(hsla(136. / 360., 0.05, 0.17, 1.), s::card_border())
                    })
                    .flex()
                    .items_center()
                    .bg(s::card_bg())
                    .when(index >= 2, |v| v.font_family("Geist Mono"))
                    .child(self.fields[index].clone()),
            )
    }
}
impl Render for CommandDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let title = if self.existing {
            format!("Edit {}", self.original.as_ref().unwrap().name)
        } else {
            "Custom ACP command".into()
        };
        let mut environment = self.field(4, "Environment", window, cx);
        if let Some(agent) = &self.original {
            if !agent.env.is_empty() && !self.clear_environment {
                environment = environment.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(4.))
                        .text_size(px(10.))
                        .line_height(px(16.))
                        .text_color(rgb(MUTED))
                        .child(format!(
                            "Saved: {}.",
                            agent.env.keys().cloned().collect::<Vec<_>>().join(", ")
                        ))
                        .child(
                            s::button(
                                "clear-acp-environment",
                                tr(cx, msg::settings::acp_agents_clear_variables),
                            )
                            .tab_index(0)
                            .h(px(16.))
                            .px_0()
                            .border_0()
                            .bg(transparent_black())
                            .text_size(px(10.))
                            .on_click(cx.listener(|this, _, _, cx| {
                                if !this.busy {
                                    this.clear_environment = true;
                                    cx.notify();
                                }
                            })),
                        ),
                );
            } else if self.clear_environment {
                environment = environment.child(
                    s::description_text(tr(cx, msg::native::saved_variables_will_be_cleared))
                        .text_size(px(10.)),
                );
            }
        }
        let body = div()
            .id("acp-command-body")
            .min_h_0()
            .overflow_y_scroll()
            .px(px(16.))
            .pt(px(4.))
            .pb(px(16.))
            .flex()
            .flex_col()
            .gap(px(12.))
            .child(self.field(1, "Name", window, cx))
            .child(self.field(2, "Executable", window, cx))
            .child(self.field(3, "Arguments", window, cx))
            .child(environment)
            .when(self.advanced, |body| {
                body.child(if self.existing {
                    div()
                        .text_size(px(11.))
                        .text_color(rgb(MUTED))
                        .child(String::from(self.original.as_ref().unwrap().id))
                } else {
                    self.field(0, "Provider ID", window, cx)
                })
                .child(self.field(5, "Fallback model IDs", window, cx))
            })
            .when_some(self.error.clone(), |body, error| {
                body.child(
                    div()
                        .id("acp-command-error")
                        .role(Role::Alert)
                        .text_size(px(11.))
                        .line_height(px(16.))
                        .text_color(s::destructive_text())
                        .child(error),
                )
            });
        let panel = div()
            .id("acp-command-dialog")
            .role(Role::Dialog)
            .aria_label(title.clone())
            .tab_group()
            .tab_stop(false)
            .track_focus(&self.root)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape" {
                    this.dismiss(cx);
                    cx.stop_propagation();
                }
                if event.keystroke.key == "tab"
                    && !event.keystroke.modifiers.control
                    && !event.keystroke.modifiers.alt
                    && !event.keystroke.modifiers.platform
                {
                    if event.keystroke.modifiers.shift {
                        window.focus_prev(cx);
                    } else {
                        window.focus_next(cx);
                    }
                    if !this.root.contains_focused(window, cx) {
                        let focus = if event.keystroke.modifiers.shift {
                            this.save.clone()
                        } else {
                            this.fields[1].read(cx).focus_handle(cx)
                        };
                        window.focus(&focus, cx);
                    }
                    cx.stop_propagation();
                }
            }))
            .w(px((f32::from(window.viewport_size().width) - 32.).min(384.)))
            .min_h_0()
            .max_h(window.viewport_size().height - px(32.))
            .flex_none()
            .rounded(px(12.))
            .when(!crate::palette::is_light(), |v| {
                v.border_1().border_color(white().opacity(0.1))
            })
            .bg(popup_bg())
            .shadow_lg()
            .text_color(rgb(TEXT))
            .flex()
            .flex_col()
            .occlude()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .px(px(16.))
                    .pt(px(16.))
                    .pb(px(10.))
                    .text_size(px(15.))
                    .line_height(px(15.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(title),
            )
            .child(body)
            .child(
                div()
                    .px(px(16.))
                    .py(px(10.))
                    .border_t_1()
                    .border_color(s::card_border().opacity(0.6))
                    .bg(s::frame_bg().opacity(0.72))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(6.))
                    .child(
                        s::button(
                            "acp-command-advanced",
                            if self.advanced {
                                ("Hide advanced").to_string()
                            } else {
                                crate::localization::text("chat-chat_composer_advanced", cx)
                            },
                        )
                        .tab_index(0)
                        .h(px(20.))
                        .px_0()
                        .border_0()
                        .bg(transparent_black())
                        .text_size(px(10.))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.advanced = !this.advanced;
                            cx.notify();
                        })),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .child(
                                s::button("cancel-acp-command", tr(cx, msg::common::cancel))
                                    .track_focus(&self.cancel)
                                    .tab_index(0)
                                    .text_size(px(11.))
                                    .px(px(13.))
                                    .border_0()
                                    .bg(transparent_black())
                                    .on_click(cx.listener(|this, _, _, cx| this.dismiss(cx))),
                            )
                            .child(
                                s::button("save-acp-command", tr(cx, msg::common::save))
                                    .track_focus(&self.save)
                                    .tab_index(if self.busy { -1 } else { 0 })
                                    .text_size(px(11.))
                                    .px(px(13.))
                                    .bg(rgb(TEXT))
                                    .text_color(rgb(crate::shell::BG))
                                    .opacity(if self.busy { 0.64 } else { 1. })
                                    .on_click(cx.listener(|this, _, _, cx| this.submit(cx))),
                            ),
                    ),
            );
        let mut bottom = div().flex_1();
        bottom.style().flex_grow = Some(3.);
        let viewport = window.viewport_size();
        deferred(
            anchored()
                .anchor(Anchor::TopLeft)
                .position(point(px(0.), px(0.)))
                .child(crate::glass::backdrop(
                    div()
                        .id("acp-command-backdrop")
                        .w(viewport.width)
                        .h(viewport.height)
                        .p(px(16.))
                        .bg(black().opacity(0.32))
                        .flex()
                        .flex_col()
                        .items_center()
                        .occlude()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                this.dismiss(cx);
                                cx.stop_propagation();
                            }),
                        )
                        .child(div().flex_1())
                        .child(panel)
                        .child(bottom),
                )),
        )
        .priority(3)
    }
}
