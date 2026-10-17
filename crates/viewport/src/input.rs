//! Zuse presentation over Zkit's mature shared text-editing engine.
use crate::palette::rgb;
use gpui::{prelude::*, *};
use gpui_base::input::{EditorState, InputEditorStyle, InputEvent, InputState, TextareaState};

pub struct Submitted;
impl EventEmitter<Submitted> for TextInput {}

#[derive(Clone)]
enum Editor {
    Single(Entity<InputState>),
    Multi(Entity<TextareaState>),
    Code(Entity<EditorState>),
}

pub struct TextInput {
    editor: Editor,
    window: AnyWindowHandle,
    light_style: bool,
    _events: Subscription,
}

impl TextInput {
    pub fn new(placeholder: &str, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let editor = cx.new(|cx| {
            let mut state = InputState::new(window, cx).placeholder(placeholder.to_owned());
            state.set_editor_style(editor_style());
            state
        });
        let events = cx.subscribe(&editor, |_, _, event, cx| {
            if matches!(event, InputEvent::PressEnter { shift: false, .. }) {
                cx.emit(Submitted);
            }
            cx.notify();
        });
        Self {
            editor: Editor::Single(editor),
            window: window.window_handle(),
            light_style: crate::palette::is_light(),
            _events: events,
        }
    }

    pub fn multiline(placeholder: &str, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let editor = cx.new(|cx| {
            let mut state = TextareaState::new(window, cx)
                .placeholder(placeholder.to_owned())
                .auto_grow(1, 8)
                .submit_on_enter(true);
            state.set_editor_style(editor_style());
            state
        });
        let events = cx.subscribe(&editor, |_, _, event, cx| {
            if matches!(event, InputEvent::PressEnter { shift: false, .. }) {
                cx.emit(Submitted);
            }
            cx.notify();
        });
        Self {
            editor: Editor::Multi(editor),
            window: window.window_handle(),
            light_style: crate::palette::is_light(),
            _events: events,
        }
    }

    pub fn code(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let editor = cx.new(|cx| {
            let mut state = EditorState::new(window, cx)
                .line_number(true)
                .folding(false)
                .soft_wrap(false);
            state.set_editor_style(editor_style());
            state
        });
        let events = cx.subscribe(&editor, |_, _, _: &InputEvent, cx| cx.notify());
        Self {
            editor: Editor::Code(editor),
            window: window.window_handle(),
            light_style: crate::palette::is_light(),
            _events: events,
        }
    }

    pub fn set_readonly(&mut self, readonly: bool, cx: &mut Context<Self>) {
        if let Editor::Code(state) = &self.editor {
            state.update(cx, |state, cx| state.set_readonly(readonly, cx));
        }
    }

    pub fn value(&self, cx: &App) -> String {
        match &self.editor {
            Editor::Single(state) => state.read(cx).value().to_string(),
            Editor::Multi(state) => state.read(cx).value().to_string(),
            Editor::Code(state) => state.read(cx).value().to_string(),
        }
    }

    pub fn focus(&mut self, cx: &mut Context<Self>) {
        let focus = self.focus_handle(cx);
        let window = self.window;
        cx.defer(move |cx| {
            let _ = cx.update_window(window, move |_, window, cx| window.focus(&focus, cx));
        });
    }
    pub fn set_value(&mut self, value: String, cx: &mut Context<Self>) {
        let editor = self.editor.clone();
        let window = self.window;
        cx.defer(move |cx| {
            let _ = cx.update_window(window, move |_, window, cx| match editor {
                Editor::Single(state) => {
                    state.update(cx, |state, cx| state.set_value(value, window, cx))
                }
                Editor::Multi(state) => {
                    state.update(cx, |state, cx| state.set_value(value, window, cx))
                }
                Editor::Code(state) => {
                    state.update(cx, |state, cx| state.set_value(value, window, cx))
                }
            });
        });
        cx.notify();
    }
}

impl Focusable for TextInput {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        match &self.editor {
            Editor::Single(state) => state.read(cx).focus_handle(cx),
            Editor::Multi(state) => state.read(cx).focus_handle(cx),
            Editor::Code(state) => state.read(cx).focus_handle(cx),
        }
    }
}

impl Render for TextInput {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let light = crate::palette::is_light();
        if self.light_style != light {
            self.light_style = light;
            match &self.editor {
                Editor::Single(state) => state.update(cx, |state, _| {
                    state.set_editor_style(editor_style());
                }),
                Editor::Multi(state) => state.update(cx, |state, _| {
                    state.set_editor_style(editor_style());
                }),
                Editor::Code(state) => state.update(cx, |state, _| {
                    state.set_editor_style(editor_style());
                }),
            }
        }
        div()
            .w_full()
            .when(matches!(self.editor, Editor::Code(_)), |d| d.h_full())
            .text_size(px(if matches!(self.editor, Editor::Single(_)) {
                12.
            } else {
                13.
            }))
            .line_height(px(if matches!(self.editor, Editor::Single(_)) {
                16.
            } else {
                22.
            }))
            .text_color(rgb(crate::shell::TEXT))
            .child(match &self.editor {
                Editor::Single(state) => state.clone().into_any_element(),
                Editor::Multi(state) => state.clone().into_any_element(),
                Editor::Code(state) => state.clone().into_any_element(),
            })
    }
}

pub fn bind_keys(cx: &mut App) {
    gpui_base::init(cx);
}

fn editor_style() -> InputEditorStyle {
    InputEditorStyle {
        foreground: rgb(crate::shell::TEXT).into(),
        muted_foreground: rgb(crate::shell::MUTED).into(),
        background: rgb(0x121413).into(),
        border: rgb(crate::shell::BORDER).into(),
        caret: rgb(crate::shell::TEXT).into(),
        selection: rgba(0xaabbee44).into(),
        editor_active_line: Some(rgba(0xffffff08).into()),
        editor_gutter_background: Some(rgb(0x101010).into()),
        ..Default::default()
    }
}
