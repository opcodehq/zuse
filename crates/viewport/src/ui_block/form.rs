//! Forms, their fields, and legacy follow-up buttons. Field values live in
//! the view per Form path; text fields keep their own editor state.

use super::layout::{js_number, snap_slider};
use super::{UiBlockPalette, UiBlockView, eid, heading, icon, tabular, width_probe};
use gpui::{prelude::*, *};
use gpui_base::input::{InputEditorStyle, InputEvent, InputState, TextareaState};
use std::cell::Cell;
use std::rc::Rc;
use zuse_ui_block::{Component, FieldValue, FollowUp, default_field_values, form_submission_text};

/// Opacity of locked controls.
const DISABLED_OPACITY: f32 = 0.64;

/// A text field's editor state.
pub(super) struct TextField {
    editor: Editor,
    disabled: Cell<bool>,
}

enum Editor {
    Single(Entity<InputState>),
    Multi(Entity<TextareaState>),
}

impl TextField {
    fn new(editor: Editor) -> Self {
        Self {
            editor,
            disabled: Cell::new(false),
        }
    }

    pub(super) fn set_style(&self, style: InputEditorStyle, cx: &mut App) {
        match &self.editor {
            Editor::Single(state) => state.update(cx, |state, cx| {
                state.set_editor_style(style);
                cx.notify();
            }),
            Editor::Multi(state) => state.update(cx, |state, cx| {
                state.set_editor_style(style);
                cx.notify();
            }),
        }
    }

    fn value(&self, cx: &App) -> String {
        match &self.editor {
            Editor::Single(state) => state.read(cx).value().to_string(),
            Editor::Multi(state) => state.read(cx).value().to_string(),
        }
    }

    fn set_disabled(&self, disabled: bool, cx: &mut App) {
        if self.disabled.replace(disabled) == disabled {
            return;
        }
        match &self.editor {
            Editor::Single(state) => state.update(cx, |state, cx| state.set_disabled(disabled, cx)),
            Editor::Multi(state) => state.update(cx, |state, cx| state.set_disabled(disabled, cx)),
        }
    }

    fn element(&self) -> AnyElement {
        match &self.editor {
            Editor::Single(state) => state.clone().into_any_element(),
            Editor::Multi(state) => state.clone().into_any_element(),
        }
    }
}

/// Editor colours for text fields: placeholder at 72% of muted text, the
/// field's own fill painted by its container.
pub(super) fn editor_style(p: &UiBlockPalette) -> InputEditorStyle {
    InputEditorStyle {
        foreground: p.foreground,
        muted_foreground: p.muted_foreground.opacity(0.72),
        background: transparent_black(),
        border: p.input,
        caret: p.foreground,
        selection: p.primary.opacity(0.28),
        ..Default::default()
    }
}

fn field_key(form: &str, name: &str) -> String {
    format!("{form}\u{1f}{name}")
}

#[derive(Clone)]
struct SliderDrag {
    key: String,
    form: String,
    name: String,
    min: f64,
    max: f64,
    step: f64,
}

struct DragGhost;

impl Render for DragGhost {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        Empty
    }
}

fn field_label(p: &UiBlockPalette, label: &str) -> Div {
    div()
        .text_size(px(12.))
        .line_height(px(16.))
        .text_color(p.muted_foreground)
        .child(label.to_string())
}

fn labeled(p: &UiBlockPalette, label: &str, control: impl IntoElement) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .min_w_0()
        .gap(px(6.))
        .child(field_label(p, label))
        .child(control)
        .into_any_element()
}

/// An outline button: border, card fill, muted on hover. `selected` marks
/// the chosen radio option (primary border and tint).
fn outline_button(
    p: &UiBlockPalette,
    id: ElementId,
    disabled: bool,
    selected: bool,
) -> Stateful<Div> {
    let (border, bg, hover_border, hover_bg) = if selected {
        let primary = p.primary;
        (primary, primary.opacity(0.15), primary, primary.opacity(0.2))
    } else {
        (p.border, p.card, p.input, p.muted)
    };
    div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .gap(px(6.))
        .h(px(28.))
        .px(px(10.))
        .rounded(px(7.))
        .border_1()
        .border_color(border)
        .bg(bg)
        .text_size(px(12.))
        .line_height(px(16.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(p.foreground)
        .role(Role::Button)
        .when(disabled, |d| d.opacity(DISABLED_OPACITY))
        .when(!disabled, |d| {
            d.cursor_pointer()
                .tab_index(0)
                .hover(move |style| style.bg(hover_bg).border_color(hover_border))
        })
}

impl UiBlockView {
    fn field_value(&self, form: &str, name: &str) -> Option<&FieldValue> {
        self.forms
            .get(form)?
            .iter()
            .rev()
            .find(|(field, _)| field == name)
            .map(|(_, value)| value)
    }

    pub(super) fn set_field_value(&mut self, form: &str, name: &str, value: FieldValue) {
        let values = self.forms.entry(form.to_string()).or_default();
        match values.iter_mut().find(|(field, _)| field == name) {
            Some((_, slot)) => *slot = value,
            None => values.push((name.to_string(), value)),
        }
    }

    pub(super) fn submit_form(&mut self, form: &Component, path: &str, cx: &mut Context<Self>) {
        if self.disabled() {
            return;
        }
        let prefix = field_key(path, "");
        let typed: Vec<(String, String)> = self
            .text_fields
            .iter()
            .filter_map(|(key, field)| {
                let name = key.strip_prefix(&prefix)?;
                Some((name.to_string(), field.value(cx)))
            })
            .collect();
        for (name, value) in typed {
            self.set_field_value(path, &name, FieldValue::Text(value));
        }
        let values = self.forms.get(path).cloned().unwrap_or_default();
        self.send(form_submission_text(form, &values), cx);
    }

    pub(super) fn form(
        &mut self,
        node: &Component,
        path: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Component::Form {
            children,
            submit_label,
            title,
        } = node
        else {
            return Empty.into_any_element();
        };
        let p = self.palette.clone();
        self.forms
            .entry(path.to_string())
            .or_insert_with(|| default_field_values(node));
        let body = self.children(children, path, Some(path), window, cx);
        let disabled = self.disabled();
        let sent = self.status == super::Status::Sent;
        let label = if sent {
            crate::localization::message(cx, |l| {
                zuse_i18n::messages::chat::message_row_ui_spec_sent(l)
            })
        } else {
            submit_label.clone().unwrap_or_else(|| {
                crate::localization::message(cx, |l| {
                    zuse_i18n::messages::chat::message_row_ui_spec_submit(l)
                })
            })
        };
        let form_node = node.clone();
        let key = path.to_string();
        let hover_bg = p.primary.opacity(0.9);
        let submit = div()
            .id(eid(path, "submit"))
            .flex()
            .flex_none()
            .items_center()
            .gap(px(6.))
            .h(px(28.))
            .px(px(15.))
            .rounded(px(7.))
            .border_1()
            .border_color(p.primary)
            .bg(p.primary)
            .text_color(p.primary_foreground)
            .text_size(px(12.))
            .line_height(px(16.))
            .font_weight(FontWeight::MEDIUM)
            .role(Role::Button)
            .aria_label(label.clone())
            .when(sent, |d| d.child(icon("check", 14.0, p.primary_foreground)))
            .child(label)
            .when(disabled, |d| d.opacity(DISABLED_OPACITY))
            .when(!disabled, |d| {
                d.cursor_pointer()
                    .tab_index(0)
                    .hover(move |style| style.bg(hover_bg))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.submit_form(&form_node, &key, cx);
                    }))
            });
        div()
            .flex()
            .flex_col()
            .min_w_0()
            .max_w(px(512.))
            .gap(px(14.))
            .when_some(title.as_ref(), |d, title| d.child(heading(&p, title)))
            .children(body)
            .child(div().flex().child(submit))
            .into_any_element()
    }

    pub(super) fn field(
        &mut self,
        node: &Component,
        path: &str,
        form: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        // Fields only validate inside a Form; a stray one still renders,
        // keyed to itself.
        let form = form.unwrap_or(path).to_string();
        let p = self.palette.clone();
        match node {
            Component::Input {
                name,
                label,
                placeholder,
                ..
            } => {
                let control = self.text_field(&form, name, false, placeholder.as_deref(), window, cx);
                labeled(&p, label, control)
            }
            Component::TextArea {
                name,
                label,
                placeholder,
                ..
            } => {
                let control = self.text_field(&form, name, true, placeholder.as_deref(), window, cx);
                labeled(&p, label, control)
            }
            Component::Select {
                name,
                label,
                options,
                ..
            } => {
                let control = self.select(&form, name, label, options, path, cx);
                labeled(&p, label, control)
            }
            Component::RadioGroup {
                name,
                label,
                options,
                ..
            } => self.radio_group(&form, name, label, options, path, cx),
            Component::Checkbox { name, label, .. } => {
                self.checkbox(&form, name, label, path, cx)
            }
            Component::Slider {
                name,
                label,
                min,
                max,
                step,
                ..
            } => self.slider(&form, name, label, *min, *max, step.unwrap_or(1.0), path, cx),
            _ => Empty.into_any_element(),
        }
    }

    fn text_field(
        &mut self,
        form: &str,
        name: &str,
        multi: bool,
        placeholder: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = self.palette.clone();
        let key = field_key(form, name);
        if !self.text_fields.contains_key(&key) {
            let initial = match self.field_value(form, name) {
                Some(FieldValue::Text(text)) => text.clone(),
                _ => String::new(),
            };
            let placeholder = placeholder.unwrap_or_default().to_string();
            let style = editor_style(&p);
            let (form_key, field_name) = (form.to_string(), name.to_string());
            let field = if multi {
                let state = cx.new(|cx| {
                    let mut state = TextareaState::new(window, cx)
                        .placeholder(placeholder)
                        .auto_grow(2, 12)
                        .default_value(initial);
                    state.set_editor_style(style);
                    state
                });
                self._subscriptions.push(cx.subscribe(
                    &state,
                    move |this, state, event: &InputEvent, cx| {
                        if matches!(event, InputEvent::Change) {
                            let value = state.read(cx).value().to_string();
                            this.set_field_value(&form_key, &field_name, FieldValue::Text(value));
                        }
                    },
                ));
                TextField::new(Editor::Multi(state))
            } else {
                let state = cx.new(|cx| {
                    let mut state = InputState::new(window, cx)
                        .placeholder(placeholder)
                        .default_value(initial);
                    state.set_editor_style(style);
                    state
                });
                self._subscriptions.push(cx.subscribe(
                    &state,
                    move |this, state, event: &InputEvent, cx| {
                        if matches!(event, InputEvent::Change) {
                            let value = state.read(cx).value().to_string();
                            this.set_field_value(&form_key, &field_name, FieldValue::Text(value));
                        }
                    },
                ));
                TextField::new(Editor::Single(state))
            };
            self.text_fields.insert(key.clone(), field);
        }
        let disabled = self.disabled();
        let field = &self.text_fields[&key];
        field.set_disabled(disabled, cx);
        let editor = field.element();
        div()
            .flex()
            .min_w_0()
            .rounded(px(7.))
            .border_1()
            .border_color(p.input)
            .bg(p.card)
            .px(px(9.))
            .text_size(px(12.))
            .text_color(p.foreground)
            .map(|d| {
                if multi {
                    d.flex_col().min_h(px(56.)).py(px(6.)).line_height(px(18.))
                } else {
                    d.items_center().h(px(28.)).line_height(px(16.))
                }
            })
            .when(disabled, |d| d.opacity(DISABLED_OPACITY))
            .child(div().w_full().min_w_0().child(editor))
            .into_any_element()
    }

    fn select(
        &mut self,
        form: &str,
        name: &str,
        label: &str,
        options: &[String],
        path: &str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = self.palette.clone();
        let disabled = self.disabled();
        let current = match self.field_value(form, name) {
            Some(FieldValue::Text(text)) => text.clone(),
            _ => String::new(),
        };
        let open = !disabled && self.open_select.as_deref() == Some(path);
        let (trigger_width, width_cell) = self.width_of(&format!("{path}#trigger"));
        let trigger_bounds = self
            .chart_bounds
            .entry(format!("{path}#trigger"))
            .or_default()
            .clone();
        let toggle_key = path.to_string();
        let measured = trigger_bounds.clone();
        let trigger = div()
            .id(eid(path, "select"))
            .relative()
            .flex()
            .items_center()
            .justify_between()
            .gap(px(6.))
            .h(px(28.))
            .min_w(px(112.))
            .rounded(px(7.))
            .border_1()
            .border_color(p.input)
            .bg(p.card)
            .px(px(9.))
            .text_size(px(12.))
            .line_height(px(16.))
            .text_color(p.foreground)
            .role(Role::ComboBox)
            .aria_label(label.to_string())
            .aria_expanded(open)
            .child(width_probe(width_cell, cx.entity_id()))
            .child(
                canvas(move |bounds, _, _| measured.set(bounds), |_, _, _, _| {})
                    .absolute()
                    .inset_0(),
            )
            .child(div().flex_1().min_w_0().truncate().child(current.clone()))
            .child(icon("alt-arrow-down", 14.0, p.foreground).opacity(0.8).mr(px(-4.)))
            .when(disabled, |d| d.opacity(DISABLED_OPACITY))
            .when(!disabled, |d| {
                d.cursor_pointer()
                    .tab_index(0)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.open_select = if this.open_select.as_deref() == Some(&toggle_key) {
                            None
                        } else {
                            Some(toggle_key.clone())
                        };
                        cx.notify();
                    }))
            });
        let menu = open.then(|| {
            let close_bounds = trigger_bounds.clone();
            let items = options.iter().enumerate().map(|(index, option)| {
                let selected = *option == current;
                let (form, name, value) = (form.to_string(), name.to_string(), option.clone());
                let hover_bg = p.muted;
                div()
                    .id(eid(path, &format!("option{index}")))
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .min_h(px(24.))
                    .py(px(2.))
                    .pl(px(6.))
                    .pr(px(12.))
                    .rounded(px(7.))
                    .text_size(px(12.))
                    .line_height(px(16.))
                    .text_color(p.foreground)
                    .cursor_pointer()
                    .role(Role::ListBoxOption)
                    .aria_selected(selected)
                    .hover(move |style| style.bg(hover_bg))
                    .child(
                        div()
                            .flex_none()
                            .w(px(14.))
                            .when(selected, |d| d.child(icon("check", 14.0, p.foreground))),
                    )
                    .child(div().min_w_0().child(option.clone()))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.set_field_value(&form, &name, FieldValue::Text(value.clone()));
                        this.open_select = None;
                        cx.notify();
                    }))
            });
            div().absolute().top(px(32.)).left_0().child(deferred(
                anchored().snap_to_window_with_margin(px(8.)).child(
                    div()
                        .id(eid(path, "menu"))
                        .occlude()
                        .flex()
                        .flex_col()
                        .min_w(px(trigger_width.max(112.)))
                        .max_h(px(280.))
                        .overflow_y_scroll()
                        .p(px(4.))
                        .rounded(px(10.))
                        .border_1()
                        .border_color(p.border.opacity(0.8))
                        .bg(p.popover)
                        .shadow_md()
                        .font_family(p.font_sans.clone())
                        .role(Role::ListBox)
                        .on_mouse_down_out(cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                            // A press on the trigger toggles it closed itself.
                            if close_bounds.get().contains(&event.position) {
                                return;
                            }
                            this.open_select = None;
                            cx.notify();
                        }))
                        .children(items),
                ),
            ))
        });
        div()
            .relative()
            .flex()
            .child(trigger)
            .children(menu)
            .into_any_element()
    }

    fn radio_group(
        &mut self,
        form: &str,
        name: &str,
        label: &str,
        options: &[String],
        path: &str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = self.palette.clone();
        let disabled = self.disabled();
        let current = match self.field_value(form, name) {
            Some(FieldValue::Text(text)) => text.clone(),
            _ => String::new(),
        };
        let buttons = options.iter().enumerate().map(|(index, option)| {
            let selected = *option == current;
            let (form, name, value) = (form.to_string(), name.to_string(), option.clone());
            outline_button(&p, eid(path, &format!("radio{index}")), disabled, selected)
                .role(Role::RadioButton)
                .aria_selected(selected)
                .aria_label(option.clone())
                .when(selected, |d| d.child(icon("check", 14.0, p.foreground)))
                .child(option.clone())
                .when(!disabled, |d| {
                    d.on_click(cx.listener(move |this, _, _, cx| {
                        this.set_field_value(&form, &name, FieldValue::Text(value.clone()));
                        cx.notify();
                    }))
                })
        });
        div()
            .flex()
            .flex_col()
            .min_w_0()
            .gap(px(6.))
            .child(field_label(&p, label))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(6.))
                    .id(eid(path, "radios"))
                    .role(Role::RadioGroup)
                    .aria_label(label.to_string())
                    .children(buttons),
            )
            .into_any_element()
    }

    fn checkbox(
        &mut self,
        form: &str,
        name: &str,
        label: &str,
        path: &str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = self.palette.clone();
        let disabled = self.disabled();
        let checked = matches!(self.field_value(form, name), Some(FieldValue::Bool(true)));
        let (form, name) = (form.to_string(), name.to_string());
        div()
            .id(eid(path, "checkbox"))
            .flex()
            .items_center()
            .gap(px(8.))
            .min_w_0()
            .text_size(px(14.))
            .line_height(px(20.))
            .text_color(p.foreground.opacity(0.9))
            .role(Role::CheckBox)
            .aria_label(label.to_string())
            .aria_toggled(if checked {
                Toggled::True
            } else {
                Toggled::False
            })
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_center()
                    .size(px(16.))
                    .rounded(px(4.))
                    .border_1()
                    .map(|d| {
                        if checked {
                            d.border_color(p.primary)
                                .bg(p.primary)
                                .child(icon("check", 12.0, p.primary_foreground))
                        } else {
                            d.border_color(p.input).bg(p.background)
                        }
                    }),
            )
            .child(div().min_w_0().child(label.to_string()))
            .when(disabled, |d| d.opacity(DISABLED_OPACITY))
            .when(!disabled, |d| {
                d.cursor_pointer()
                    .tab_index(0)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.set_field_value(&form, &name, FieldValue::Bool(!checked));
                        cx.notify();
                    }))
            })
            .into_any_element()
    }

    fn set_slider(&mut self, drag: &SliderDrag, value: f64, cx: &mut Context<Self>) {
        if self.disabled() {
            return;
        }
        let value = snap_slider(value, drag.min, drag.max, drag.step);
        if self.field_value(&drag.form, &drag.name) != Some(&FieldValue::Number(value)) {
            self.set_field_value(&drag.form, &drag.name, FieldValue::Number(value));
            cx.notify();
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn slider(
        &mut self,
        form: &str,
        name: &str,
        label: &str,
        min: f64,
        max: f64,
        step: f64,
        path: &str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = self.palette.clone();
        let disabled = self.disabled();
        let value = match self.field_value(form, name) {
            Some(FieldValue::Number(value)) => *value,
            _ => min,
        };
        let span = (max - min).max(f64::EPSILON);
        let fraction = ((value - min) / span).clamp(0.0, 1.0) as f32;
        let drag = SliderDrag {
            key: path.to_string(),
            form: form.to_string(),
            name: name.to_string(),
            min,
            max,
            step,
        };
        let bounds = Rc::new(Cell::new(Bounds::<Pixels>::default()));
        let measured = bounds.clone();
        let (track, range, thumb_border) = (p.input, p.primary, p.input);
        let thumb_fill = hsla(0.0, 0.0, 1.0, 1.0);
        let value_at = |drag: &SliderDrag, x: Pixels, bounds: Bounds<Pixels>| {
            let t = (f32::from(x - bounds.left()) / f32::from(bounds.size.width).max(1.0))
                .clamp(0.0, 1.0) as f64;
            drag.min + (drag.max - drag.min) * t
        };
        let press = drag.clone();
        let keys = drag.clone();
        let own_key = path.to_string();
        let control = div()
            .id(eid(path, "slider"))
            .relative()
            .h(px(16.))
            .w_full()
            .role(Role::Slider)
            .aria_label(label.to_string())
            .aria_numeric_value(value)
            .aria_min_numeric_value(min)
            .aria_max_numeric_value(max)
            .aria_numeric_value_step(step)
            .child(
                canvas(
                    move |b, _, _| measured.set(b),
                    move |b, _, window, _| {
                        let y = b.center().y;
                        let line = Bounds::new(point(b.left(), y - px(2.)), size(b.size.width, px(4.)));
                        window.paint_quad(quad(
                            line,
                            px(2.),
                            track,
                            px(0.),
                            transparent_black(),
                            BorderStyle::default(),
                        ));
                        window.paint_quad(quad(
                            Bounds::new(line.origin, size(b.size.width * fraction, px(4.))),
                            px(2.),
                            range,
                            px(0.),
                            transparent_black(),
                            BorderStyle::default(),
                        ));
                        window.paint_quad(quad(
                            Bounds::centered_at(
                                point(b.left() + b.size.width * fraction, y),
                                size(px(16.), px(16.)),
                            ),
                            px(8.),
                            thumb_fill,
                            px(1.),
                            thumb_border,
                            BorderStyle::default(),
                        ));
                    },
                )
                .size_full(),
            )
            .when(disabled, |d| d.opacity(DISABLED_OPACITY))
            .when(!disabled, |d| {
                d.cursor_pointer()
                    .tab_index(0)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                            let value = value_at(&press, event.position.x, bounds.get());
                            this.set_slider(&press, value, cx);
                        }),
                    )
                    .on_key_down(cx.listener(move |this, event: &KeyDownEvent, window, cx| {
                        if event.keystroke.modifiers.modified() {
                            return;
                        }
                        let next = match event.keystroke.key.as_str() {
                            "right" | "up" => value + keys.step,
                            "left" | "down" => value - keys.step,
                            "pageup" => value + 10.0 * keys.step,
                            "pagedown" => value - 10.0 * keys.step,
                            "home" => keys.min,
                            "end" => keys.max,
                            _ => return,
                        };
                        this.set_slider(&keys, next, cx);
                        window.prevent_default();
                        cx.stop_propagation();
                    }))
                    .on_click(|_, _, cx| cx.stop_propagation())
                    .on_drag(drag.clone(), |_, _, _, cx| cx.new(|_| DragGhost))
                    .on_drag_move(cx.listener(
                        move |this, event: &DragMoveEvent<SliderDrag>, _, cx| {
                            let drag = event.drag(cx).clone();
                            if drag.key != own_key {
                                return;
                            }
                            let value = value_at(&drag, event.event.position.x, event.bounds);
                            this.set_slider(&drag, value, cx);
                        },
                    ))
            });
        div()
            .flex()
            .flex_col()
            .min_w_0()
            .gap(px(8.))
            .child(
                div()
                    .flex()
                    .items_baseline()
                    .justify_between()
                    .gap(px(8.))
                    .child(field_label(&p, label))
                    .child(
                        div()
                            .text_size(px(12.))
                            .line_height(px(16.))
                            .font_features(tabular())
                            .text_color(p.foreground)
                            .child(js_number(value)),
                    ),
            )
            .child(control)
            .into_any_element()
    }

    pub(super) fn follow_ups(
        &mut self,
        items: &[FollowUp],
        path: &str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = self.palette.clone();
        let disabled = self.disabled();
        let chosen = self
            .chosen_follow_up
            .as_ref()
            .filter(|(chosen, _)| chosen == path)
            .map(|(_, index)| *index);
        div()
            .flex()
            .flex_wrap()
            .gap(px(6.))
            .min_w_0()
            .children(items.iter().enumerate().map(|(index, item)| {
                let (key, prompt) = (path.to_string(), item.prompt.clone());
                outline_button(&p, eid(path, &format!("follow{index}")), disabled, false)
                    .flex_shrink(1.)
                    .max_w_full()
                    .aria_label(item.label.clone())
                    .when(disabled && chosen == Some(index), |d| {
                        d.child(icon("check", 14.0, p.foreground))
                    })
                    .child(div().min_w_0().truncate().child(item.label.clone()))
                    .when(!disabled, |d| {
                        d.on_click(cx.listener(move |this, _, _, cx| {
                            if this.disabled() {
                                return;
                            }
                            this.chosen_follow_up = Some((key.clone(), index));
                            this.send(prompt.clone(), cx);
                        }))
                    })
            }))
            .into_any_element()
    }
}
