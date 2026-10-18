//! Zuse presentation over Zkit's mature shared text-editing engine.
use crate::palette::rgb;
use gpui::{prelude::*, *};
use gpui_base::input::{EditorState, InputEditorStyle, InputEvent, InputState, TextareaState};
pub(crate) mod edit_menu;

pub struct Submitted;
impl EventEmitter<Submitted> for TextInput {}

pub const CLIPBOARD_PASTE_ERROR: &str =
    "Clipboard access is unavailable. Use the keyboard paste shortcut.";

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
    clipboard_unavailable: bool,
    menu: Option<edit_menu::EditMenu>,
    _events: Subscription,
}

impl TextInput {
    pub fn new(placeholder: &str, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let owner = cx.weak_entity();
        let editor = cx.new(|cx| {
            let mut state = InputState::new(window, cx).placeholder(placeholder.to_owned());
            state.set_editor_style(editor_style());
            state.on_context_menu(edit_menu::handler(owner, state.focus_handle(cx)));
            state
        });
        let events = cx.subscribe(&editor, |this, _, event, cx| this.on_input_event(event, cx));
        Self {
            editor: Editor::Single(editor),
            window: window.window_handle(),
            light_style: crate::palette::is_light(),
            clipboard_unavailable: false,
            menu: None,
            _events: events,
        }
    }

    pub fn multiline(placeholder: &str, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let owner = cx.weak_entity();
        let editor = cx.new(|cx| {
            let mut state = TextareaState::new(window, cx)
                .placeholder(placeholder.to_owned())
                .auto_grow(1, 8)
                .submit_on_enter(true);
            state.set_editor_style(editor_style());
            state.on_context_menu(edit_menu::handler(owner, state.focus_handle(cx)));
            state
        });
        let events = cx.subscribe(&editor, |this, _, event, cx| this.on_input_event(event, cx));
        Self {
            editor: Editor::Multi(editor),
            window: window.window_handle(),
            light_style: crate::palette::is_light(),
            clipboard_unavailable: false,
            menu: None,
            _events: events,
        }
    }

    pub fn code(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let owner = cx.weak_entity();
        let editor = cx.new(|cx| {
            let mut state = EditorState::new(window, cx)
                .line_number(true)
                .folding(false)
                .soft_wrap(false);
            state.set_editor_style(editor_style());
            state.on_context_menu(edit_menu::handler(owner, state.focus_handle(cx)));
            state
        });
        let events = cx.subscribe(&editor, |this, _, event, cx| this.on_input_event(event, cx));
        Self {
            editor: Editor::Code(editor),
            window: window.window_handle(),
            light_style: crate::palette::is_light(),
            clipboard_unavailable: false,
            menu: None,
            _events: events,
        }
    }

    pub fn set_readonly(&mut self, readonly: bool, cx: &mut Context<Self>) {
        let was_readonly = match &self.editor {
            Editor::Single(state) => state.read(cx).context_menu_capabilities().is_readonly(),
            Editor::Multi(state) => state.read(cx).context_menu_capabilities().is_readonly(),
            Editor::Code(state) => state.read(cx).context_menu_capabilities().is_readonly(),
        };
        if was_readonly != readonly {
            self.close_edit_menu(cx);
        }
        match &self.editor {
            Editor::Single(state) => state.update(cx, |state, cx| state.set_readonly(readonly, cx)),
            Editor::Multi(state) => state.update(cx, |state, cx| state.set_readonly(readonly, cx)),
            Editor::Code(state) => state.update(cx, |state, cx| state.set_readonly(readonly, cx)),
        }
    }

    pub fn clipboard_error(&self) -> Option<&'static str> {
        self.clipboard_unavailable.then_some(CLIPBOARD_PASTE_ERROR)
    }

    fn on_input_event(&mut self, event: &InputEvent, cx: &mut Context<Self>) {
        if matches!(event, InputEvent::Change) {
            self.close_edit_menu(cx);
        }
        match event {
            InputEvent::ClipboardReadFailed => self.clipboard_unavailable = true,
            InputEvent::Change | InputEvent::Focus => self.clipboard_unavailable = false,
            _ => {}
        }
        if !matches!(self.editor, Editor::Code(_))
            && matches!(event, InputEvent::PressEnter { shift: false, .. })
        {
            cx.emit(Submitted);
        }
        cx.notify();
    }

    fn close_edit_menu(&mut self, cx: &mut Context<Self>) {
        if self.menu.take().is_some() {
            self.focus(cx);
            cx.notify();
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
        self.close_edit_menu(cx);
        self.clipboard_unavailable = false;
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
            .when(
                matches!(self.editor, Editor::Code(_)) && self.clipboard_unavailable,
                |d| d.pb(px(18.)),
            )
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
            .when(self.clipboard_unavailable, |d| {
                d.child(
                    div()
                        .id("clipboard-paste-error")
                        .role(gpui::Role::Alert)
                        .text_size(px(11.))
                        .line_height(px(14.))
                        .text_color(rgb(0xf87171))
                        .child(CLIPBOARD_PASTE_ERROR),
                )
            })
            .when_some(self.menu.as_ref(), |d, menu| {
                d.child(edit_menu::render(menu, cx))
            })
    }
}

pub fn bind_keys(cx: &mut App) {
    gpui_base::init(cx);
}

impl edit_menu::Host for TextInput {
    fn edit_menu(&mut self) -> &mut Option<edit_menu::EditMenu> {
        &mut self.menu
    }
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
