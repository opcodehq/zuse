//! Shared ACP rows and their keyboard-accessible action menu.
use crate::{
    acp_catalog::{popup_bg, spinner},
    assets::icon,
    palette::rgb,
    settings_style as s,
    shell::{MUTED, TEXT},
};
use gpui::{prelude::*, *};
use serde_json::Value;
use std::{cell::Cell, collections::HashMap, rc::Rc};
use zuse_protocol::{HarnessId, custom_acp::CustomAcpAgent};

#[derive(Clone, PartialEq)]
pub enum Probe {
    Testing,
    Ready,
    Error(String),
}
#[derive(Clone, Default, PartialEq)]
pub struct Data {
    pub scope: u64,
    pub agents: Vec<CustomAcpAgent>,
    pub loaded: bool,
    pub advanced: bool,
    pub runtime: Vec<Value>,
    pub blocked: bool,
    pub busy: Option<String>,
    pub error: Option<String>,
    pub probes: HashMap<HarnessId, Probe>,
}
#[derive(Clone)]
pub enum Action {
    Add,
    Edit(HarnessId),
    Remove(HarnessId),
    Duplicate(HarnessId),
    Toggle(HarnessId, bool),
    Probe(HarnessId),
    Advanced(HarnessId),
}
pub struct Event {
    pub scope: u64,
    pub action: Action,
}
actions!(acp_agent_menu, [Previous, Next, First, Last, Choose, Close]);
struct Bindings;
impl Global for Bindings {}
struct Trigger {
    focus: FocusHandle,
    bounds: Rc<Cell<Bounds<Pixels>>>,
}
struct Menu {
    agent: HarnessId,
    selected: usize,
}
pub struct AgentRows {
    data: Data,
    triggers: HashMap<HarnessId, Trigger>,
    menu: Option<Menu>,
    menu_focus: FocusHandle,
    add_focus: FocusHandle,
}
impl EventEmitter<Event> for AgentRows {}
impl AgentRows {
    pub fn new(cx: &mut Context<Self>) -> Self {
        if !cx.has_global::<Bindings>() {
            cx.set_global(Bindings);
            cx.bind_keys([
                KeyBinding::new("up", Previous, Some("AcpAgentMenu")),
                KeyBinding::new("down", Next, Some("AcpAgentMenu")),
                KeyBinding::new("home", First, Some("AcpAgentMenu")),
                KeyBinding::new("end", Last, Some("AcpAgentMenu")),
                KeyBinding::new("enter", Choose, Some("AcpAgentMenu")),
                KeyBinding::new("space", Choose, Some("AcpAgentMenu")),
                KeyBinding::new("escape", Close, Some("AcpAgentMenu")),
                KeyBinding::new("tab", Close, Some("AcpAgentMenu")),
                KeyBinding::new("shift-tab", Close, Some("AcpAgentMenu")),
            ]);
        }
        Self {
            data: Data::default(),
            triggers: HashMap::new(),
            menu: None,
            menu_focus: cx.focus_handle(),
            add_focus: cx.focus_handle().tab_stop(true),
        }
    }
    pub fn sync(&mut self, data: Data, cx: &mut Context<Self>) {
        if data == self.data {
            return;
        }
        if data.scope != self.data.scope
            || data.blocked
            || self
                .menu
                .as_ref()
                .is_some_and(|menu| !data.agents.iter().any(|agent| agent.id == menu.agent))
        {
            self.menu = None;
        }
        self.triggers
            .retain(|id, _| data.agents.iter().any(|agent| agent.id == *id));
        self.data = data;
        cx.notify();
    }
    fn runtime(&self, id: HarnessId) -> Option<&Value> {
        let id = String::from(id);
        self.data.runtime.iter().find(|row| row["id"] == id)
    }
    fn enabled(&self, id: HarnessId) -> bool {
        self.runtime(id).is_some_and(|row| {
            row["enabled"]
                .as_bool()
                .unwrap_or(row["installed"] != false)
        })
    }
    fn testable(&self, id: HarnessId) -> bool {
        self.enabled(id) && !matches!(self.data.probes.get(&id), Some(Probe::Testing))
    }
    fn close_menu(&mut self, restore: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(menu) = self.menu.take() else {
            return;
        };
        if let Some(trigger) = self.triggers.get(&menu.agent) {
            let trigger = trigger.focus.clone();
            if restore {
                window.focus(&trigger, cx);
            } else {
                let owner = cx.weak_entity();
                let handle = window.window_handle();
                cx.defer(move |cx| {
                    let _ = cx.update_window(handle, |_, window, cx| {
                        let _ = owner.update(cx, |this, cx| {
                            if this.menu.is_none() && this.menu_focus.is_focused(window) {
                                window.focus(&trigger, cx);
                            }
                        });
                    });
                });
            }
        }
        cx.notify();
    }
    pub fn dismiss(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.menu.is_none() {
            return false;
        }
        self.close_menu(true, window, cx);
        true
    }
    fn toggle_menu(&mut self, id: HarnessId, window: &mut Window, cx: &mut Context<Self>) {
        if self.data.blocked {
            return;
        }
        if self.menu.as_ref().is_some_and(|menu| menu.agent == id) {
            self.close_menu(true, window, cx);
            return;
        }
        self.menu = Some(Menu {
            agent: id,
            selected: if self.testable(id) { 0 } else { 1 },
        });
        window.focus(&self.menu_focus, cx);
        cx.notify();
    }
    fn select(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(menu) = self.menu.as_ref() else {
            return;
        };
        let id = menu.agent;
        if index == 0 && !self.testable(id) {
            return;
        }
        let action = match index {
            0 => Action::Probe(id),
            1 => Action::Edit(id),
            2 => Action::Duplicate(id),
            3 if self.data.advanced => Action::Advanced(id),
            _ => Action::Remove(id),
        };
        self.close_menu(true, window, cx);
        if index == if self.data.advanced { 4 } else { 3 } {
            window.focus(&self.add_focus, cx);
        }
        cx.emit(Event {
            scope: self.data.scope,
            action,
        });
    }
    fn navigate(&mut self, step: isize, cx: &mut Context<Self>) {
        let Some(menu) = self.menu.as_ref() else {
            return;
        };
        let can_test = self.testable(menu.agent);
        let menu = self.menu.as_mut().unwrap();
        let count = if self.data.advanced { 5 } else { 4 };
        for _ in 0..count {
            menu.selected = (menu.selected as isize + step).rem_euclid(count) as usize;
            if menu.selected != 0 || can_test {
                break;
            }
        }
        cx.notify();
    }
    fn menu(&self, window: &Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let menu = self.menu.as_ref()?;
        let id = menu.agent;
        let bounds = self.triggers.get(&id)?.bounds.get();
        let menu_height = if self.data.advanced { 159. } else { 131. };
        let (anchor, at) =
            if bounds.bottom() + px(menu_height + 12.) <= window.viewport_size().height {
                (
                    Anchor::TopRight,
                    point(bounds.right(), bounds.bottom() + px(4.)),
                )
            } else {
                (
                    Anchor::BottomRight,
                    point(bounds.right(), bounds.top() - px(4.)),
                )
            };
        let selected = menu.selected;
        let mut labels = vec!["Test connection", "Edit", "Duplicate"];
        if self.data.advanced {
            labels.push("Advanced settings");
        }
        labels.push("Remove");
        let last = labels.len() - 1;
        let width = labels
            .iter()
            .map(|label| {
                let mut font = window.text_style().font();
                font.weight = FontWeight::NORMAL;
                f32::from(
                    window
                        .text_system()
                        .shape_line(
                            (*label).into(),
                            px(12.),
                            &[TextRun {
                                len: label.len(),
                                font,
                                color: window.text_style().color,
                                background_color: None,
                                underline: None,
                                strikethrough: None,
                            }],
                            None,
                        )
                        .width(),
                ) + 22.
            })
            .fold(112., f32::max);
        let can_test = self.testable(id);
        let card = div()
            .id("acp-agent-actions-menu")
            .role(Role::Menu)
            .aria_label("Agent actions")
            .key_context("AcpAgentMenu")
            .track_focus(&self.menu_focus)
            .w(px(width))
            .p(px(4.))
            .rounded(px(8.))
            .border_1()
            .border_color(white().opacity(0.1))
            .bg(popup_bg())
            .text_color(rgb(TEXT))
            .shadow_lg()
            .occlude()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_down_out(
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    if !bounds.contains(&event.position) {
                        this.close_menu(false, window, cx);
                    }
                }),
            )
            .on_action(cx.listener(|this, _: &Previous, _, cx| this.navigate(-1, cx)))
            .on_action(cx.listener(|this, _: &Next, _, cx| this.navigate(1, cx)))
            .on_action(cx.listener(|this, _: &First, _, cx| {
                if let Some(menu) = this.menu.as_mut() {
                    menu.selected = if this.data.advanced { 4 } else { 3 };
                }
                this.navigate(1, cx);
            }))
            .on_action(cx.listener(|this, _: &Last, _, cx| {
                if let Some(menu) = this.menu.as_mut() {
                    menu.selected = 0;
                }
                this.navigate(-1, cx);
            }))
            .on_action(cx.listener(|this, _: &Choose, window, cx| {
                if let Some(menu) = &this.menu {
                    this.select(menu.selected, window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &Close, window, cx| this.close_menu(true, window, cx)))
            .children(labels.into_iter().enumerate().map(|(index, label)| {
                let enabled = index != 0 || can_test;
                div()
                    .when(index == last, |v| {
                        v.child(div().h(px(1.)).my(px(4.)).bg(s::card_border()))
                    })
                    .child(
                        div()
                            .id(("acp-agent-action", index))
                            .role(Role::MenuItem)
                            .aria_label(label)
                            .h(px(28.))
                            .px(px(6.))
                            .rounded(px(6.))
                            .flex()
                            .items_center()
                            .text_size(px(12.))
                            .text_color(if index == last {
                                s::destructive_text()
                            } else {
                                rgb(TEXT).into()
                            })
                            .when(!enabled, |v| v.opacity(0.64).cursor_default())
                            .when(enabled, |v| {
                                v.cursor_pointer()
                                    .when(selected == index, |v| v.bg(s::frame_bg()))
                                    .hover(|v| v.bg(s::frame_bg()))
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.select(index, window, cx)
                                    }))
                            }),
                    )
            }));
        Some(
            deferred(anchored().anchor(anchor).position(at).child(card))
                .priority(4)
                .into_any_element(),
        )
    }
}
impl Render for AgentRows {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let menu = self.menu(window, cx);
        let scope = self.data.scope;
        let actions = div()
            .flex()
            .items_center()
            .gap(px(6.))
            .when_some(self.data.busy.clone(), |v, label| {
                v.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .text_size(px(10.))
                        .text_color(rgb(MUTED))
                        .child(spinner("acp-operation-spinner", 12.))
                        .child(label),
                )
            })
            .child(
                s::button("add-acp-agent", "")
                    .track_focus(&self.add_focus)
                    .tab_index(0)
                    .px(px(12.))
                    .rounded(px(6.))
                    .text_size(px(12.))
                    .when(self.data.blocked || !self.data.loaded, |v| {
                        v.opacity(0.64).cursor_default()
                    })
                    .aria_label("Add")
                    .child(icon("plus").size(px(14.)))
                    .child("Add")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if !this.data.blocked && this.data.loaded {
                            cx.emit(Event {
                                scope,
                                action: Action::Add,
                            });
                        }
                    })),
            );
        let agents = self.data.agents.clone();
        let rows = agents
            .into_iter()
            .enumerate()
            .map(|(index, agent)| {
                let id = agent.id;
                let enabled = self.enabled(id);
                let runtime = self.runtime(id);
                let installed = runtime.is_some_and(|row| row["installed"] != false);
                let interactive = !self.data.blocked && runtime.is_some() && (enabled || installed);
                let probe = self.data.probes.get(&id);
                let (color, label) = if runtime.is_none() {
                    (
                        MUTED,
                        if matches!(probe, Some(Probe::Testing)) {
                            "Setting up…"
                        } else {
                            "Connection not tested"
                        }
                        .to_owned(),
                    )
                } else if !enabled {
                    (MUTED, "Disabled".to_owned())
                } else {
                    match probe {
                        Some(Probe::Testing) => (MUTED, "Testing connection…".into()),
                        Some(Probe::Ready) => (0x10b981, "Ready".into()),
                        Some(Probe::Error(error)) => (0xef4444, error.clone()),
                        None => (MUTED, "Connection not tested".into()),
                    }
                };
                let trigger = self.triggers.entry(id).or_insert_with(|| Trigger {
                    focus: cx.focus_handle().tab_stop(true),
                    bounds: Rc::new(Cell::new(Bounds::default())),
                });
                let focus = trigger.focus.clone();
                let bounds = trigger.bounds.clone();
                div()
                    .id(("acp-agent-row", index))
                    .min_h(px(48.))
                    .px(px(12.))
                    .py(px(8.))
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .child(
                        div()
                            .size(px(28.))
                            .flex_none()
                            .rounded(px(6.))
                            .bg(s::frame_bg().opacity(0.6))
                            .flex()
                            .items_center()
                            .justify_center()
                            .when(!enabled, |v| v.opacity(0.6))
                            .child(icon("bot").size(px(16.)).text_color(rgb(TEXT))),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .when(!enabled, |v| v.opacity(0.6))
                            .child(s::title_text(agent.name.clone()).truncate())
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(6.))
                                    .child(
                                        div()
                                            .size(px(6.))
                                            .flex_none()
                                            .rounded_full()
                                            .bg(rgb(color)),
                                    )
                                    .child(s::description_text(label).truncate()),
                            ),
                    )
                    .child(
                        s::toggle(
                            ("enable-acp-agent", index),
                            enabled,
                            &format!("Enable {}", agent.name),
                        )
                        .tab_index(if interactive { 0 } else { -1 })
                        .when(!interactive, |v| v.opacity(0.5).cursor_default())
                        .on_click(cx.listener(move |_, _, _, cx| {
                            if interactive {
                                cx.emit(Event {
                                    scope,
                                    action: Action::Toggle(id, !enabled),
                                });
                            }
                        })),
                    )
                    .child(
                        s::button(("acp-agent-actions", index), "")
                            .relative()
                            .size(px(28.))
                            .px_0()
                            .border_0()
                            .bg(transparent_black())
                            .track_focus(&focus)
                            .tab_index(0)
                            .aria_label(format!("Actions for {}", agent.name))
                            .child(icon("more-horizontal").size(px(16.)))
                            .when(self.data.blocked, |v| v.opacity(0.64).cursor_default())
                            .child(
                                canvas(move |rect, _, _| bounds.set(rect), |_, _, _, _| {})
                                    .absolute()
                                    .size_full(),
                            )
                            .on_click(cx.listener(move |this, _, window, cx| {
                                if this.data.scope == scope {
                                    this.toggle_menu(id, window, cx)
                                }
                            })),
                    )
                    .into_any_element()
            })
            .collect::<Vec<_>>();
        s::frame()
            .child(s::header_with_action(
                "ACP agents",
                "",
                Some(actions.into_any_element()),
            ))
            .child(
                s::card()
                    .when(self.data.agents.is_empty(), |v| {
                        v.child(
                            div()
                                .px(px(12.))
                                .py(px(12.))
                                .text_size(px(11.))
                                .line_height(px(14.85))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(rgb(TEXT).opacity(0.85))
                                .text_center()
                                .child("No ACP agents yet"),
                        )
                    })
                    .children(rows)
                    .when_some(self.data.error.clone(), |v, error| {
                        v.child(
                            s::description_text(error)
                                .text_color(s::destructive_text())
                                .px(px(12.))
                                .py(px(8.)),
                        )
                    }),
            )
            .children(menu)
    }
}
