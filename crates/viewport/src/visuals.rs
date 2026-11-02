//! Inline host-owned visual trees. A control targets the revision the reader
//! saw; disconnect and navigation retire subscriptions instead of replaying input.
use super::*;
use crate::localization::message as tr;
use crate::shell::{BORDER, FAINT, MUTED, TEXT, column, control, icon_button, label, row};
use std::time::Duration;
use std::{cell::Cell, rc::Rc, sync::Arc};
use web_time::Instant;
use zuse_i18n::messages as msg;
use zuse_protocol::visual_session as wire;

#[derive(Default)]
pub(super) struct Visuals {
    views: HashMap<String, Visual>,
    requests: HashMap<u64, (String, Request)>,
    timer: Option<Task<()>>,
}
struct Visual {
    owner: u64,
    lease: Option<String>,
    revision: u64,
    tree: Value,
    error: Option<String>,
    busy: bool,
    queued_slider: Option<(String, Value, Value)>,
    slider_target: Option<(String, f64)>,
    canvases: HashMap<String, CanvasView>,
    started: Instant,
}
enum Request {
    Owner,
    Control,
    Draw { path: String, revision: u64 },
}
struct CanvasView {
    handler: u64,
    height: f32,
    animate: bool,
    bounds: Rc<Cell<Bounds<Pixels>>>,
    pointer: Rc<Cell<Option<[f32; 3]>>>,
    painted: Rc<Cell<Option<Instant>>>,
    commands: Arc<Vec<Value>>,
    signature: Option<(f32, f32, Option<[f32; 3]>)>,
    requested: Instant,
    pending: bool,
    error: Option<String>,
}

#[derive(Clone)]
struct SliderDrag {
    visual: String,
    path: String,
    min: f64,
    max: f64,
    step: f64,
}
struct DragGhost;
impl Render for DragGhost {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        gpui::Empty
    }
}
impl WorkspaceView {
    pub(super) fn reset_visuals(&mut self) {
        let requests = std::mem::take(&mut self.visuals.requests);
        self.visuals.views.clear();
        self.visuals.timer = None;
        for id in requests.into_keys() {
            self.cancel(id);
        }
    }
    fn open_visual(&mut self, id: String, cx: &mut Context<Self>) {
        self.close_visual(&id);
        if !self.connected || self.visuals.views.len() >= 8 {
            return;
        }
        let Some(chat) = &self.selected else {
            return;
        };
        let params = json!({"chatId":chat["id"],"visualId":id,"targetDeviceId":chat["deviceId"]});
        let owner = self.call(methods::OPEN_VISUAL, params);
        self.visuals
            .requests
            .insert(owner, (id.clone(), Request::Owner));
        self.visuals.views.insert(
            id,
            Visual {
                owner,
                lease: None,
                revision: 0,
                tree: Value::Null,
                error: None,
                busy: false,
                queued_slider: None,
                slider_target: None,
                canvases: HashMap::new(),
                started: Instant::now(),
            },
        );
        if self.visuals.timer.is_none() {
            self.visuals.timer = Some(cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor()
                        .timer(Duration::from_millis(100))
                        .await;
                    if !this
                        .update(cx, |this, cx| {
                            this.refresh_visual_draws(cx);
                            !this.visuals.views.is_empty()
                        })
                        .unwrap_or(false)
                    {
                        break;
                    }
                }
            }));
        }
        cx.notify();
    }
    fn close_visual(&mut self, id: &str) {
        let ids: Vec<_> = self
            .visuals
            .requests
            .iter()
            .filter(|(_, v)| v.0 == id)
            .map(|(id, _)| *id)
            .collect();
        for request in ids {
            self.visuals.requests.remove(&request);
            self.cancel(request);
        }
        self.visuals.views.remove(id);
        if self.visuals.views.is_empty() {
            self.visuals.timer = None;
        }
    }
    fn visual_control(
        &mut self,
        id: &str,
        path: &str,
        revision: u64,
        argument: Value,
        cx: &mut Context<Self>,
    ) {
        if !self.connected {
            return;
        }
        let Some(view) = self.visuals.views.get_mut(id) else {
            return;
        };
        if view.error.is_some() || view.revision != revision {
            return;
        }
        let Some(node) = at(&view.tree, path) else {
            return;
        };
        let Some((handler, kind)) = input(node) else {
            return;
        };
        if kind == "Slider" {
            view.slider_target = argument["value"].as_f64().map(|value| (path.into(), value));
        }
        if view.busy {
            if kind == "Slider" {
                view.queued_slider = Some((path.into(), argument, node.clone()));
            }
            return;
        }
        if kind != "Slider" {
            view.slider_target = None;
        }
        if kind == "Slider"
            && argument["value"]
                .as_f64()
                .is_some_and(|value| Some(value) == node["kind"]["Slider"]["value"].as_f64())
        {
            view.slider_target = None;
            return;
        }
        let Some(lease) = view.lease.clone() else {
            return;
        };
        view.busy = true;
        let target = self
            .selected
            .as_ref()
            .map(|v| v["deviceId"].clone())
            .unwrap_or_default();
        let request=self.call(methods::CONTROL_VISUAL,json!({"lease":lease,"revision":revision,"handler":handler,"argument":argument,"targetDeviceId":target}));
        self.visuals
            .requests
            .insert(request, (id.into(), Request::Control));
        cx.notify();
    }
    pub(super) fn receive_visual(
        &mut self,
        id: u64,
        frame: &Value,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some((visual, kind)) = self.visuals.requests.get(&id) else {
            return false;
        };
        let visual = visual.clone();
        let owner = matches!(kind, Request::Owner);
        if let Request::Draw { path, revision } = kind {
            let (path, revision) = (path.clone(), *revision);
            if frame.get("ok").is_none() && frame.get("err").is_none() && frame["done"] != true {
                return true;
            }
            self.visuals.requests.remove(&id);
            self.pending.remove(&id);
            if let Some(view) = self
                .visuals
                .views
                .get_mut(&visual)
                .filter(|v| v.revision == revision)
            {
                if let Some(canvas) = view.canvases.get_mut(&path) {
                    canvas.pending = false;
                    if let Some(commands) = frame["ok"]["commands"]
                        .as_array()
                        .filter(|_| frame["ok"]["revision"].as_u64() == Some(revision))
                    {
                        canvas.commands = Arc::new(commands.clone());
                        canvas.error = None;
                    } else {
                        canvas.error = Some("The canvas could not draw this frame.".into());
                    }
                }
            }
            cx.notify();
            return true;
        }
        if frame.get("err").is_some() || frame["done"] == true {
            if let Some(view) = self.visuals.views.get_mut(&visual) {
                view.error=Some(if owner {"Visual session ended. Reopen it to start again."}else{"The control response was interrupted. Reopen the visual before trying again."}.into());
                view.busy = false;
                view.queued_slider = None;
                view.slider_target = None;
            }
            let requests: Vec<_> = self
                .visuals
                .requests
                .iter()
                .filter(|(_, v)| v.0 == visual)
                .map(|(id, _)| *id)
                .collect();
            for request in requests {
                self.visuals.requests.remove(&request);
                self.cancel(request);
            }
            cx.notify();
            return true;
        }
        if owner {
            let Some(item) = frame.get("item") else {
                return true;
            };
            let parsed = serde_json::from_value::<wire::Frame>(item.clone())
                .ok()
                .filter(|v| {
                    v.version == wire::VERSION && v.lease.len() <= 128 && v.tree.is_object()
                });
            let Some(view) = self.visuals.views.get_mut(&visual) else {
                return true;
            };
            if let Some(value) = parsed.filter(|v| {
                view.lease.as_ref().is_none_or(|lease| lease == &v.lease)
                    && (view.lease.is_none() || v.revision == view.revision + 1)
            }) {
                view.lease = Some(value.lease);
                view.revision = value.revision;
                view.tree = value.tree;
                view.busy = false;
                view.canvases.clear();
                collect_canvases(&view.tree, "", &mut view.canvases);
                let next = view.queued_slider.take();
                view.slider_target = None;
                let revision = view.revision;
                if let Some((path, argument, previous)) = next {
                    if at(&view.tree, &path).is_some_and(|node| same_slider(node, &previous)) {
                        self.visual_control(&visual, &path, revision, argument, cx);
                    }
                }
            } else {
                view.error = Some(
                    item["error"]
                        .as_str()
                        .unwrap_or("The host returned an invalid visual frame.")
                        .chars()
                        .take(2000)
                        .collect(),
                );
                view.busy = false;
                view.queued_slider = None;
                view.slider_target = None;
                let owner = view.owner;
                self.visuals.requests.remove(&owner);
                self.cancel(owner);
            }
        } else if frame.get("ok").is_some() {
            self.visuals.requests.remove(&id);
            self.pending.remove(&id);
            // Only the canonical owner tree unlocks the next interaction.
        }
        cx.notify();
        true
    }
    pub(super) fn visual_part(&self, part: &Value, cx: &mut Context<Self>) -> AnyElement {
        let id = part["id"].as_str().unwrap_or_default().to_owned();
        let title = part["title"].as_str().unwrap_or("Visual").to_owned();
        let view = self.visuals.views.get(&id);
        let heading = row()
            .gap_2()
            .child(
                div()
                    .text_size(px(13.))
                    .font_weight(FontWeight::MEDIUM)
                    .child(title),
            )
            .child(div().flex_1())
            .when(view.is_none(), |d| {
                d.child(
                    control(
                        SharedString::from(format!("open-{id}")),
                        tr(cx, msg::native::open_visual),
                    )
                    .on_click({
                        let id = id.clone();
                        cx.listener(move |this, _, _, cx| this.open_visual(id.clone(), cx))
                    }),
                )
            })
            .when(view.is_some(), |d| {
                d.child(
                    icon_button(
                        SharedString::from(format!("close-{id}")),
                        "close",
                        "Close visual",
                    )
                    .on_click({
                        let id = id.clone();
                        cx.listener(move |this, _, _, cx| {
                            this.close_visual(&id);
                            cx.notify();
                        })
                    }),
                )
            });
        let mut frame = column()
            .id(SharedString::from(format!("visual-{id}")))
            .tab_group()
            .tab_stop(false)
            .on_key_down(|event: &KeyDownEvent, window, cx| {
                if event.keystroke.key == "tab" {
                    if event.keystroke.modifiers.shift {
                        window.focus_prev(cx);
                    } else {
                        window.focus_next(cx);
                    }
                    cx.stop_propagation();
                }
            })
            .w_full()
            .min_w_0()
            .gap_3()
            .py_3()
            .child(heading);
        if let Some(view) = view {
            if let Some(error) = &view.error {
                frame = frame.child(label(error.clone())).child(
                    control(
                        SharedString::from(format!("retry-{id}")),
                        tr(cx, msg::native::reopen_visual),
                    )
                    .on_click({
                        let id = id.clone();
                        cx.listener(move |this, _, _, cx| this.open_visual(id.clone(), cx))
                    }),
                );
            } else if view.tree.is_null() {
                frame = frame.child(label(tr(cx, msg::native::opening_visual)));
            } else {
                frame = frame.child(self.visual_node(
                    &id,
                    "",
                    &view.tree,
                    view.revision,
                    view.busy,
                    cx,
                ));
            }
        }
        frame.into_any_element()
    }
    fn visual_node(
        &self,
        id: &str,
        path: &str,
        node: &Value,
        revision: u64,
        busy: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(kind) = node["kind"]
            .as_object()
            .and_then(|v| v.keys().next())
            .map(String::as_str)
            .or_else(|| node["kind"].as_str())
        else {
            return div().into_any_element();
        };
        let value = &node["kind"][kind];
        let child = |index: usize, node: &Value, cx: &mut Context<Self>| {
            self.visual_node(id, &format!("{path}/{index}"), node, revision, busy, cx)
        };
        let clickable = |element: Stateful<Div>, argument: Value, cx: &mut Context<Self>| {
            let (id, path) = (id.to_owned(), path.to_owned());
            element
                .tab_index(0)
                .tab_stop(!busy)
                .focus_visible(|style| style.bg(rgb(0x89ca21).opacity(0.12)))
                .on_key_down(|event: &KeyDownEvent, _, cx| {
                    if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                        cx.stop_propagation();
                    }
                })
                .opacity(if busy { 0.65 } else { 1. })
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.visual_control(&id, &path, revision, argument.clone(), cx)
                }))
                .into_any_element()
        };
        let element = match kind {
            "Stack" => {
                let mut stack = div().flex().gap(px(number(&value["gap"], 8.))).min_w_0();
                if value["axis"] == "Horizontal" {
                    stack = stack.flex_row();
                } else {
                    stack = stack.flex_col();
                }
                if value["wrap"] == true {
                    stack = stack.flex_wrap();
                }
                stack = match value["align"].as_str() {
                    Some("Center") => stack.items_center(),
                    Some("End") => stack.items_end(),
                    _ => stack,
                };
                stack = match value["justify"].as_str() {
                    Some("Center") => stack.justify_center(),
                    Some("End") => stack.justify_end(),
                    Some("Between") => stack.justify_between(),
                    _ => stack,
                };
                stack = stack.children(
                    value["children"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .enumerate()
                        .map(|(i, n)| child(i, n, cx))
                        .collect::<Vec<_>>(),
                );
                if value["on_click"].is_u64() {
                    clickable(
                        stack
                            .id(SharedString::from(format!("{id}-{path}")))
                            .role(Role::Button)
                            .aria_label(tr(cx, msg::native::interactive_group))
                            .cursor_pointer(),
                        json!({"kind":"none"}),
                        cx,
                    )
                } else {
                    stack.into_any_element()
                }
            }
            "Grid" => div()
                .grid()
                .grid_cols(value["columns"].as_u64().unwrap_or(1).min(12) as u16)
                .gap(px(number(&value["gap"], 8.)))
                .children(
                    value["children"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .enumerate()
                        .map(|(i, n)| child(i, n, cx))
                        .collect::<Vec<_>>(),
                )
                .into_any_element(),
            "Text" => {
                let spans = value["spans"].as_array().cloned().unwrap_or_default();
                let mut text = String::new();
                let mut runs = vec![];
                for span in spans {
                    let content = span["text"].as_str().unwrap_or_default();
                    text.push_str(content);
                    let mut font = gpui::font(if span["mono"] == true || value["mono"] == true {
                        "Geist Mono"
                    } else {
                        "Geist"
                    });
                    font.weight = weight(if span["weight"].is_null() {
                        &value["weight"]
                    } else {
                        &span["weight"]
                    });
                    if span["italic"] == true || value["italic"] == true {
                        font.style = FontStyle::Italic;
                    }
                    runs.push(TextRun {
                        len: content.len(),
                        font,
                        color: color(if span["color"].is_null() {
                            value.get("color")
                        } else {
                            span.get("color")
                        }),
                        background_color: None,
                        underline: None,
                        strikethrough: None,
                    });
                }
                let mut text = div()
                    .text_size(px(text_size(&value["size"])))
                    .child(StyledText::new(SharedString::from(text)).with_runs(runs));
                text = match value["align"].as_str() {
                    Some("Center") => text.text_center(),
                    Some("End") => text.text_right(),
                    _ => text,
                };
                if value["truncate"] == true {
                    text = text.text_ellipsis();
                }
                text.into_any_element()
            }
            "Stat" => column()
                .gap_1()
                .child(label(
                    value["label"].as_str().unwrap_or_default().to_owned(),
                ))
                .child(
                    row()
                        .gap_2()
                        .child(
                            div()
                                .text_size(px(28.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(value["value"].as_str().unwrap_or_default().to_owned()),
                        )
                        .when_some(value["delta"].as_str().map(str::to_owned), |d, delta| {
                            d.child(label(delta).text_color(color(value.get("tone"))))
                        }),
                )
                .into_any_element(),
            "Rule" => div().h(px(1.)).w_full().bg(rgb(BORDER)).into_any_element(),
            "Space" => div()
                .h(px(number(value, 8.).clamp(0., 720.)))
                .into_any_element(),
            "Button" => clickable(
                control(
                    SharedString::from(format!("{id}-{path}")),
                    value["label"].as_str().unwrap_or("Button").to_owned(),
                )
                .when(value["primary"] == true, |d| {
                    d.bg(rgb(0x89ca21)).text_color(rgb(0x14180e))
                }),
                json!({"kind":"none"}),
                cx,
            ),
            "Toggle" => {
                let checked = value["value"].as_bool().unwrap_or(false);
                clickable(
                    row()
                        .id(SharedString::from(format!("{id}-{path}")))
                        .role(Role::Switch)
                        .aria_toggled(if checked {
                            Toggled::True
                        } else {
                            Toggled::False
                        })
                        .aria_label(value["label"].as_str().unwrap_or("Toggle").to_owned())
                        .cursor_pointer()
                        .gap_2()
                        .child(
                            div()
                                .w(px(30.))
                                .h(px(18.))
                                .rounded_full()
                                .bg(rgb(if checked { 0x89ca21 } else { 0x555b53 }))
                                .flex()
                                .items_center()
                                .when(checked, |d| d.justify_end())
                                .p(px(2.))
                                .child(div().size(px(14.)).rounded_full().bg(rgb(0xf5f7f2))),
                        )
                        .child(label(
                            value["label"].as_str().unwrap_or_default().to_owned(),
                        )),
                    json!({"kind":"bool","value":!checked}),
                    cx,
                )
            }
            "Tabs" => row()
                .id(SharedString::from(format!("{id}-{path}-tabs")))
                .role(Role::TabList)
                .aria_label(tr(cx, msg::native::visual_tabs))
                .gap_2()
                .flex_wrap()
                .children(
                    value["options"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_str)
                        .map(|option| {
                            clickable(
                                control(
                                    SharedString::from(format!("{id}-{path}-{option}")),
                                    option.to_owned(),
                                )
                                .role(Role::Tab)
                                .aria_selected(value["value"] == option)
                                .when(value["value"] == option, |d| {
                                    d.bg(rgb(0x32372f)).text_color(rgb(TEXT))
                                }),
                                json!({"kind":"text","value":option}),
                                cx,
                            )
                        })
                        .collect::<Vec<_>>(),
                )
                .into_any_element(),
            "Slider" => self.visual_slider(id, path, value, revision, busy, cx),
            "Table" => {
                let columns = value["columns"]
                    .as_array()
                    .map(Vec::as_slice)
                    .unwrap_or(&[]);
                let rows = value["rows"].as_array().map(Vec::as_slice).unwrap_or(&[]);
                let count = columns.len().max(
                    rows.iter()
                        .filter_map(Value::as_array)
                        .map(Vec::len)
                        .max()
                        .unwrap_or(0),
                );
                let cell = |index: usize| {
                    let column = columns.get(index).unwrap_or(&Value::Null);
                    let base = div().min_w_0().px_2().py_2().flex();
                    let base =
                        if let Some(width) = column["width"].as_f64().filter(|v| v.is_finite()) {
                            base.flex_none().w(px((width as f32).clamp(0., 4096.)))
                        } else {
                            base.flex_1().flex_basis(px(0.))
                        };
                    match column["align"].as_str() {
                        Some("Center") => base.justify_center(),
                        Some("End") => base.justify_end(),
                        _ => base,
                    }
                };
                let mut table = column()
                    .id(SharedString::from(format!("{id}-{path}-table")))
                    .role(Role::Table)
                    .w_full();
                if columns
                    .iter()
                    .any(|c| c["label"].as_str().is_some_and(|s| !s.is_empty()))
                {
                    table = table.child(
                        row()
                            .id(SharedString::from(format!("{id}-{path}-header")))
                            .role(Role::Row)
                            .items_stretch()
                            .children(
                                (0..count)
                                    .map(|c| {
                                        cell(c)
                                            .id(SharedString::from(format!(
                                                "{id}-{path}-header-{c}"
                                            )))
                                            .role(Role::ColumnHeader)
                                            .child(
                                                label(
                                                    columns
                                                        .get(c)
                                                        .and_then(|c| c["label"].as_str())
                                                        .unwrap_or_default()
                                                        .to_owned(),
                                                )
                                                .font_weight(FontWeight::MEDIUM),
                                            )
                                    })
                                    .collect::<Vec<_>>(),
                            ),
                    );
                }
                for (r, cells) in rows.iter().enumerate() {
                    if r > 0 || !columns.is_empty() {
                        table = table.child(div().h(px(1.)).w_full().bg(rgb(BORDER)));
                    }
                    table = table.child(
                        row()
                            .id(SharedString::from(format!("{id}-{path}-row-{r}")))
                            .role(Role::Row)
                            .items_stretch()
                            .when(value["hover"] == true, |view| {
                                view.hover(|style| style.bg(rgb(crate::shell::RAISED)))
                            })
                            .children(
                                (0..count)
                                    .map(|c| {
                                        let content = cells
                                            .get(c)
                                            .map(|node| {
                                                self.visual_node(
                                                    id,
                                                    &format!("{path}/{r}/{c}"),
                                                    node,
                                                    revision,
                                                    busy,
                                                    cx,
                                                )
                                            })
                                            .unwrap_or_else(|| div().into_any_element());
                                        cell(c)
                                            .id(SharedString::from(format!(
                                                "{id}-{path}-cell-{r}-{c}"
                                            )))
                                            .role(Role::Cell)
                                            .child(content)
                                    })
                                    .collect::<Vec<_>>(),
                            ),
                    );
                }
                table.into_any_element()
            }
            "Chart" => chart(value),
            "Link" => {
                let url = value["url"].as_str().unwrap_or_default().to_owned();
                control(
                    SharedString::from(format!("{id}-{path}")),
                    value["label"].as_str().unwrap_or("Open link").to_owned(),
                )
                .tab_index(0)
                .role(Role::Link)
                .focus_visible(|style| style.bg(rgb(0x89ca21).opacity(0.12)))
                .text_color(rgb(0x89ca21))
                .on_click(move |_, _, cx| {
                    if url.starts_with("https://") || url.starts_with("http://") {
                        cx.open_url(&url);
                    }
                })
                .into_any_element()
            }
            "Canvas" => self.visual_canvas(id, path, value, revision, cx),
            "Scene" | "Shader" => {
                label(format!("{kind} rendering is available in the desktop app."))
                    .into_any_element()
            }
            _ => label(tr(cx, msg::native::unsupported_visual_element)).into_any_element(),
        };
        let mut container = div().min_w_0().child(element);
        if node["grow"] == true {
            container = container.flex_1();
        }
        if let Some(width) = node["width"].as_f64() {
            container = container.w(px((width as f32).clamp(0., 4096.)));
        }
        container.into_any_element()
    }
    fn refresh_visual_draws(&mut self, cx: &mut Context<Self>) {
        if !self.connected || !self.environment.visible || self.settings || self.project_picker {
            return;
        }
        let now = Instant::now();
        let mut draws = vec![];
        for (id, view) in &mut self.visuals.views {
            if view.error.is_some() || view.busy {
                continue;
            }
            let Some(lease) = &view.lease else {
                continue;
            };
            if view.canvases.values().any(|c| c.pending) {
                continue;
            }
            let mut candidates: Vec<_> = view
                .canvases
                .iter()
                .map(|(path, canvas)| (path.clone(), canvas.requested))
                .collect();
            candidates.sort_by_key(|(_, requested)| *requested);
            for (path, _) in candidates {
                let canvas = view.canvases.get_mut(&path).unwrap();
                let bounds = canvas.bounds.get();
                let width = f32::from(bounds.size.width).clamp(0., 2048.);
                let height = canvas.height;
                if width < 1.
                    || canvas.error.is_some()
                    || canvas
                        .painted
                        .get()
                        .is_none_or(|t| now.duration_since(t) > Duration::from_millis(400))
                {
                    continue;
                }
                let pointer = canvas.pointer.get();
                let signature = (width, height, pointer);
                if canvas.signature.as_ref() == Some(&signature)
                    && (!canvas.animate
                        || cx.reduce_motion()
                        || now.duration_since(canvas.requested) < Duration::from_millis(100))
                {
                    continue;
                }
                canvas.signature = Some(signature);
                canvas.requested = now;
                canvas.pending = true;
                draws.push((id.clone(),path.clone(),view.revision,json!({"lease":lease,"revision":view.revision,"handler":canvas.handler,"width":width,"height":height,"time":view.started.elapsed().as_secs_f64().min(86400.),"pointer":pointer.map(|p|json!({"x":p[0],"y":p[1],"down":p[2]!=0.})),"targetDeviceId":self.selected.as_ref().map(|c|c["deviceId"].clone())})));
                break;
            }
        }
        for (id, path, revision, params) in draws {
            let request = self.call(methods::DRAW_VISUAL, params);
            self.visuals
                .requests
                .insert(request, (id, Request::Draw { path, revision }));
        }
    }
    fn visual_canvas(
        &self,
        id: &str,
        path: &str,
        value: &Value,
        revision: u64,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(canvas) = self
            .visuals
            .views
            .get(id)
            .and_then(|v| v.canvases.get(path))
        else {
            return label(tr(cx, msg::native::preparing_canvas)).into_any_element();
        };
        let bounds = canvas.bounds.clone();
        let painted = canvas.painted.clone();
        let pointer = canvas.pointer.clone();
        let commands = canvas.commands.clone();
        let height = canvas.height;
        let mut area = div()
            .id(SharedString::from(format!("{id}-{path}")))
            .w_full()
            .h(px(height))
            .overflow_hidden()
            .child(
                gpui::canvas(
                    move |b, _, _| {
                        bounds.set(b);
                    },
                    move |b, _, window, cx| {
                        painted.set(Some(Instant::now()));
                        paint_commands(&commands, b.origin, window, cx);
                    },
                )
                .size_full(),
            );
        let measured = canvas.bounds.clone();
        let p = pointer.clone();
        let painted = canvas.painted.clone();
        area = area.on_mouse_move(move |event, _, _| {
            let b = measured.get();
            let x = f32::from(event.position.x - b.left())
                .clamp(0., f32::from(b.size.width).min(2048.));
            let y = f32::from(event.position.y - b.top()).clamp(0., height);
            p.set(Some([
                x,
                y,
                if event.pressed_button.is_some() {
                    1.
                } else {
                    0.
                },
            ]));
            painted.set(Some(Instant::now()));
        });
        let hover_pointer = pointer.clone();
        let hover_painted = canvas.painted.clone();
        area = area.on_hover(move |hovered: &bool, _, _| {
            if !*hovered {
                hover_pointer.set(None);
                hover_painted.set(Some(Instant::now()));
            }
        });
        if value["on_click"].is_u64() {
            let (id, path, bounds) = (id.to_owned(), path.to_owned(), canvas.bounds.clone());
            area = area
                .cursor_pointer()
                .tab_index(0)
                .role(Role::Button)
                .aria_label(tr(cx, msg::native::interactive_canvas))
                .focus_visible(|style| style.bg(rgb(0x89ca21).opacity(0.12)))
                .on_key_down(|event: &KeyDownEvent, _, cx| {
                    if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                        cx.stop_propagation();
                    }
                })
                .on_click(cx.listener(move |this, event: &ClickEvent, _, cx| {
                    cx.stop_propagation();
                    let b = bounds.get();
                    let p = if matches!(event, ClickEvent::Keyboard(_)) {
                        b.center()
                    } else {
                        event.position()
                    };
                    let x = f32::from(p.x - b.left()).clamp(0., f32::from(b.size.width).min(2048.));
                    let y = f32::from(p.y - b.top()).clamp(0., height);
                    this.visual_control(
                        &id,
                        &path,
                        revision,
                        json!({"kind":"point","value":[x,y]}),
                        cx,
                    );
                }));
        } else {
            area = area.on_click(|_, _, cx| cx.stop_propagation());
        }

        column()
            .child(area)
            .when_some(canvas.error.clone(), |d, error| d.child(label(error)))
            .into_any_element()
    }
    fn visual_drag_value(&mut self, spec: &SliderDrag, value: f64, cx: &mut Context<Self>) {
        let Some(view) = self.visuals.views.get(&spec.visual) else {
            return;
        };
        let Some(node) = at(&view.tree, &spec.path) else {
            return;
        };
        let current = &node["kind"]["Slider"];
        if current["min"].as_f64() != Some(spec.min)
            || current["max"].as_f64() != Some(spec.max)
            || current["step"].as_f64() != Some(spec.step)
        {
            return;
        }
        let revision = view.revision;
        self.visual_control(
            &spec.visual,
            &spec.path,
            revision,
            json!({"kind":"number","value":value}),
            cx,
        );
    }
    fn visual_slider_key(&mut self, spec: &SliderDrag, key: &str, cx: &mut Context<Self>) -> bool {
        let Some(view) = self.visuals.views.get(&spec.visual) else {
            return false;
        };
        let Some(node) = at(&view.tree, &spec.path) else {
            return false;
        };
        let current = view
            .slider_target
            .as_ref()
            .filter(|(path, _)| path == &spec.path)
            .map(|(_, value)| *value)
            .or_else(|| node["kind"]["Slider"]["value"].as_f64())
            .unwrap_or(spec.min);
        let next = match key {
            "right" | "up" => current + spec.step,
            "left" | "down" => current - spec.step,
            "pageup" => current + 10. * spec.step,
            "pagedown" => current - 10. * spec.step,
            "home" => spec.min,
            "end" => spec.max,
            _ => return false,
        };
        self.visual_drag_value(spec, next.clamp(spec.min, spec.max), cx);
        true
    }
    fn visual_slider(
        &self,
        id: &str,
        path: &str,
        value: &Value,
        _revision: u64,
        busy: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let min = value["min"].as_f64().unwrap_or(0.);
        let max = value["max"].as_f64().unwrap_or(1.);
        let current = value["value"].as_f64().unwrap_or(min);
        let step = value["step"].as_f64().unwrap_or(1.).max(f64::EPSILON);
        let fraction = ((current - min) / (max - min).max(f64::EPSILON)).clamp(0., 1.) as f32;
        let spec = SliderDrag {
            visual: id.into(),
            path: path.into(),
            min,
            max,
            step,
        };
        let bounds = Rc::new(Cell::new(Bounds::<Pixels>::default()));
        let measured = bounds.clone();
        let press = spec.clone();
        let keys = spec.clone();
        column()
            .gap_1()
            .child(
                row()
                    .child(label(
                        value["label"].as_str().unwrap_or_default().to_owned(),
                    ))
                    .child(div().flex_1())
                    .child(label(format_number(current, &value["format"]))),
            )
            .child(
                div()
                    .id(SharedString::from(format!("{id}-{path}")))
                    .role(Role::Slider)
                    .tab_index(0)
                    .aria_numeric_value(current)
                    .aria_min_numeric_value(min)
                    .aria_max_numeric_value(max)
                    .aria_numeric_value_step(step)
                    .aria_value(format_number(current, &value["format"]))
                    .focus_visible(|style| style.bg(rgb(0x89ca21).opacity(0.12)))
                    .on_key_down(cx.listener(move |this, event: &KeyDownEvent, window, cx| {
                        if event.keystroke.modifiers.modified() {
                            return;
                        }
                        if this.visual_slider_key(&keys, &event.keystroke.key, cx) {
                            window.prevent_default();
                            cx.stop_propagation();
                        }
                    }))
                    .aria_label(value["label"].as_str().unwrap_or("Value").to_owned())
                    .cursor_pointer()
                    .h(px(28.))
                    .relative()
                    .opacity(if busy { 0.7 } else { 1. })
                    .child(
                        canvas(
                            move |b, _, _| {
                                measured.set(b);
                            },
                            move |b, _, window, _| {
                                let y = b.center().y;
                                window.paint_quad(fill(
                                    Bounds::new(
                                        point(b.left(), y - px(3.)),
                                        size(b.size.width, px(6.)),
                                    ),
                                    rgb(0x444b3c),
                                ));
                                window.paint_quad(fill(
                                    Bounds::new(
                                        point(b.left(), y - px(3.)),
                                        size(b.size.width * fraction, px(6.)),
                                    ),
                                    rgb(0x89ca21),
                                ));
                                window.paint_quad(quad(
                                    Bounds::centered_at(
                                        point(b.left() + b.size.width * fraction, y),
                                        size(px(16.), px(16.)),
                                    ),
                                    px(8.),
                                    rgb(0xf5f7f2),
                                    px(0.),
                                    transparent_black(),
                                    BorderStyle::default(),
                                ));
                            },
                        )
                        .size_full(),
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                            let value = slider_value(&press, event.position.x, bounds.get());
                            this.visual_drag_value(&press, value, cx);
                        }),
                    )
                    .on_click(|_, _, cx| cx.stop_propagation())
                    .on_drag(spec.clone(), |_, _, _, cx| cx.new(|_| DragGhost))
                    .on_drag_move(cx.listener(
                        move |this, event: &DragMoveEvent<SliderDrag>, _, cx| {
                            let spec = event.drag(cx).clone();
                            let value = slider_value(&spec, event.event.position.x, event.bounds);
                            this.visual_drag_value(&spec, value, cx);
                        },
                    )),
            )
            .into_any_element()
    }
}
fn slider_value(spec: &SliderDrag, x: Pixels, bounds: Bounds<Pixels>) -> f64 {
    let f =
        (f32::from(x - bounds.left()) / f32::from(bounds.size.width).max(1.)).clamp(0., 1.) as f64;
    (spec.min + (((spec.max - spec.min) * f) / spec.step).round() * spec.step)
        .clamp(spec.min, spec.max)
}
fn input(node: &Value) -> Option<(u64, &str)> {
    let kinds = node["kind"].as_object()?;
    for (kind, value) in kinds {
        let field = match kind.as_str() {
            "Button" | "Stack" | "Canvas" => "on_click",
            "Tabs" | "Toggle" | "Slider" => "on_change",
            _ => continue,
        };
        return Some((value[field].as_u64()?, kind));
    }
    None
}
fn at<'a>(tree: &'a Value, path: &str) -> Option<&'a Value> {
    if path.is_empty() {
        return Some(tree);
    }
    let mut node = tree;
    let mut indices = path.trim_start_matches('/').split('/');
    while let Some(index) = indices.next() {
        let index = index.parse::<usize>().ok()?;
        let kind = node["kind"].as_object()?.keys().next()?;
        node = if kind == "Table" {
            let column = indices.next()?.parse::<usize>().ok()?;
            node["kind"][kind]["rows"].get(index)?.get(column)?
        } else {
            node["kind"][kind]["children"].get(index)?
        };
    }
    Some(node)
}
fn number(v: &Value, default: f32) -> f32 {
    v.as_f64()
        .filter(|n| n.is_finite())
        .map(|n| n as f32)
        .unwrap_or(default)
}
fn text_size(v: &Value) -> f32 {
    match v.as_str() {
        Some("Xs") => 11.,
        Some("Sm") => 12.,
        Some("Lg") => 15.,
        Some("Xl") => 16.,
        Some("Xxl") => 19.,
        _ => 14.,
    }
}
fn weight(v: &Value) -> FontWeight {
    match v.as_str() {
        Some("Medium") => FontWeight::MEDIUM,
        Some("Semibold") => FontWeight::SEMIBOLD,
        Some("Bold") => FontWeight::BOLD,
        _ => FontWeight::NORMAL,
    }
}
fn color(v: Option<&Value>) -> Hsla {
    let Some(v) = v.filter(|v| !v.is_null()) else {
        return rgb(TEXT).into();
    };
    let token = &v["token"];
    let token_rgb = match token.as_str() {
        Some("Muted") => MUTED,
        Some("Faint") => FAINT,
        Some("Accent") => 0x89ca21,
        Some("OnAccent") => 0x14180e,
        Some("Success") => 0x77ba86,
        Some("Warning") => 0xe2bc70,
        Some("Danger") => 0xdf9290,
        Some("Border") => BORDER,
        _ => {
            if let Some(index) = token["Chart"].as_u64() {
                [0x89ca21, 0x69b8c9, 0xc69ee0, 0xe2b66f, 0xd78393, 0x78b6a2][index as usize % 6]
            } else {
                TEXT
            }
        }
    };
    let mut c: Hsla = rgb(token_rgb).into();
    if let Some(mix) = v["mix"].as_array().filter(|m| m.len() == 2) {
        let other = color(Some(&json!({"token":mix[0],"alpha":1.}))).to_rgb();
        let a = c.to_rgb();
        let t = number(&mix[1], 0.).clamp(0., 1.);
        let lerp = |x: f32, y: f32| x + (y - x) * t;
        c = Rgba {
            r: lerp(a.r, other.r),
            g: lerp(a.g, other.g),
            b: lerp(a.b, other.b),
            a: 1.,
        }
        .into();
    }
    c.a = v["alpha"].as_f64().unwrap_or(1.).clamp(0., 1.) as f32;
    c
}
fn format_number(n: f64, format: &Value) -> String {
    let decimals = format["decimals"]
        .as_u64()
        .unwrap_or(if n.fract() == 0. { 0 } else { 2 })
        .min(6) as usize;
    format!(
        "{}{n:.decimals$}{}",
        format["prefix"].as_str().unwrap_or_default(),
        format["unit"].as_str().unwrap_or_default()
    )
}
#[derive(Debug)]
enum Mark {
    Box([f32; 4], Value),
    Dot([f32; 2], Value),
    Line(Vec<[f32; 2]>, Value),
    Area(Vec<[f32; 2]>, Value),
}
fn chart_marks(chart: &Value, width: f32, height: f32) -> (Vec<Mark>, f64, f64) {
    let series = chart["series"].as_array().cloned().unwrap_or_default();
    let scatter = chart["kind"] == "Scatter";
    let stacked = chart["stacked"] == true;
    let horizontal = chart["horizontal"] == true && chart["kind"] == "Bar";
    let count = series
        .iter()
        .filter_map(|s| s["values"].as_array().map(Vec::len))
        .max()
        .unwrap_or(1)
        .max(1);
    let mut positive = vec![0f64; count];
    let mut negative = positive.clone();
    let mut lo = 0f64;
    let mut hi = 0f64;
    let mut xlo = f64::INFINITY;
    let mut xhi = f64::NEG_INFINITY;
    for s in &series {
        if scatter {
            for p in s["points"].as_array().into_iter().flatten() {
                if let (Some(x), Some(y)) = (p[0].as_f64(), p[1].as_f64()) {
                    xlo = xlo.min(x);
                    xhi = xhi.max(x);
                    lo = lo.min(y);
                    hi = hi.max(y);
                }
            }
        } else {
            for (i, v) in s["values"].as_array().into_iter().flatten().enumerate() {
                if let Some(v) = v.as_f64() {
                    let total = if stacked {
                        if v >= 0. {
                            positive[i] += v;
                            positive[i]
                        } else {
                            negative[i] += v;
                            negative[i]
                        }
                    } else {
                        v
                    };
                    lo = lo.min(total);
                    hi = hi.max(total);
                }
            }
        }
    }
    lo = chart["y_min"].as_f64().unwrap_or(lo);
    hi = chart["y_max"].as_f64().unwrap_or(hi);
    if hi <= lo {
        hi = lo + 1.;
    }
    if !xlo.is_finite() {
        xlo = 0.;
        xhi = 1.;
    }
    if xhi <= xlo {
        xhi = xlo + 1.;
    }
    let y = |v: f64| height - height * ((v - lo) / (hi - lo)) as f32;
    let vx = |v: f64| width * ((v - lo) / (hi - lo)) as f32;
    let mut marks = vec![];
    positive.fill(0.);
    negative.fill(0.);
    for (index, s) in series.iter().enumerate() {
        let c = s["color"].clone();
        if scatter {
            for p in s["points"].as_array().into_iter().flatten() {
                if let (Some(x), Some(v)) = (p[0].as_f64(), p[1].as_f64()) {
                    marks.push(Mark::Dot(
                        [width * ((x - xlo) / (xhi - xlo)) as f32, y(v)],
                        c.clone(),
                    ));
                }
            }
            continue;
        }
        let mut points = vec![];
        let mut bottoms = vec![];
        let flush =
            |points: &mut Vec<[f32; 2]>, bottoms: &mut Vec<[f32; 2]>, marks: &mut Vec<Mark>| {
                if chart["kind"] == "Area" && !points.is_empty() {
                    let mut fill = points.clone();
                    fill.extend(bottoms.iter().rev().copied());
                    marks.push(Mark::Area(fill, c.clone()));
                }
                if !points.is_empty() {
                    marks.push(Mark::Line(std::mem::take(points), c.clone()));
                }
                bottoms.clear();
            };
        for (i, v) in s["values"].as_array().into_iter().flatten().enumerate() {
            let Some(v) = v.as_f64() else {
                flush(&mut points, &mut bottoms, &mut marks);
                continue;
            };
            let base = if stacked {
                if v >= 0. { positive[i] } else { negative[i] }
            } else {
                0.
            };
            let total = base + v;
            if stacked {
                if v >= 0. {
                    positive[i] = total;
                } else {
                    negative[i] = total;
                }
            }
            if chart["kind"] == "Bar" {
                let slot = if horizontal { height } else { width } / count as f32;
                let group = if stacked { 1 } else { series.len().max(1) } as f32;
                let band = slot / group;
                let offset = i as f32 * slot + if stacked { 0. } else { index as f32 * band };
                if horizontal {
                    let a = vx(base.clamp(lo, hi));
                    let b = vx(total.clamp(lo, hi));
                    marks.push(Mark::Box(
                        [
                            a.min(b),
                            offset + 1.,
                            (b - a).abs().max(1.),
                            (band - 2.).max(1.),
                        ],
                        c.clone(),
                    ));
                } else {
                    let a = y(base.clamp(lo, hi));
                    let b = y(total.clamp(lo, hi));
                    marks.push(Mark::Box(
                        [
                            offset + 1.,
                            a.min(b),
                            (band - 2.).max(1.),
                            (b - a).abs().max(1.),
                        ],
                        c.clone(),
                    ));
                }
            } else {
                let x = if count == 1 {
                    width / 2.
                } else {
                    i as f32 * width / (count - 1) as f32
                };
                points.push([x, y(total)]);
                bottoms.push([x, y(base.clamp(lo, hi))]);
            }
        }
        flush(&mut points, &mut bottoms, &mut marks);
    }
    (marks, lo, hi)
}
fn chart(value: &Value) -> AnyElement {
    let chart = Arc::new(value.clone());
    let height = number(&value["height"], 200.).clamp(40., 720.);
    let series = value["series"].as_array().cloned().unwrap_or_default();
    let legend = value["legend"] == true;
    column()
        .w_full()
        .gap_2()
        .child(
            canvas(
                move |_, _, _| {},
                move |bounds, _, window, _| {
                    let width = f32::from(bounds.size.width).max(1.);
                    let height = f32::from(bounds.size.height).max(1.);
                    let (marks, _, _) = chart_marks(&chart, width, height);
                    window.paint_layer(bounds, |window| {
                        for mark in marks {
                            match mark {
                                Mark::Box([x, y, w, h], c) => window.paint_quad(fill(
                                    Bounds::new(
                                        bounds.origin + point(px(x), px(y)),
                                        size(px(w), px(h)),
                                    ),
                                    color(Some(&c)),
                                )),
                                Mark::Dot([x, y], c) => window.paint_quad(quad(
                                    Bounds::centered_at(
                                        bounds.origin + point(px(x), px(y)),
                                        size(px(6.), px(6.)),
                                    ),
                                    px(3.),
                                    color(Some(&c)),
                                    px(0.),
                                    transparent_black(),
                                    BorderStyle::default(),
                                )),
                                mark @ (Mark::Line(..) | Mark::Area(..)) => {
                                    let area = matches!(&mark, Mark::Area(..));
                                    let (points, c) = match mark {
                                        Mark::Line(p, c) | Mark::Area(p, c) => (p, c),
                                        _ => unreachable!(),
                                    };
                                    let mut path = if area {
                                        PathBuilder::fill()
                                    } else {
                                        PathBuilder::stroke(px(2.))
                                    };
                                    for (i, [x, y]) in points.iter().enumerate() {
                                        let at = bounds.origin + point(px(*x), px(*y));
                                        if i == 0 {
                                            path.move_to(at);
                                        } else {
                                            path.line_to(at);
                                        }
                                    }
                                    if area {
                                        path.close();
                                    }
                                    if let Ok(path) = path.build() {
                                        let mut c = color(Some(&c));
                                        if area {
                                            c.a *= 0.25;
                                        }
                                        window.paint_path(path, c);
                                    }
                                }
                            }
                        }
                    });
                },
            )
            .w_full()
            .h(px(height)),
        )
        .when(legend, |d| {
            d.child(
                row().gap_3().flex_wrap().children(
                    series
                        .iter()
                        .map(|s| {
                            row()
                                .gap_1()
                                .child(div().size(px(6.)).rounded_full().bg(color(s.get("color"))))
                                .child(label(s["name"].as_str().unwrap_or_default().to_owned()))
                        })
                        .collect::<Vec<_>>(),
                ),
            )
        })
        .child(
            row().justify_between().children(
                value["x"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .take(16)
                    .map(|s| label(s.to_owned()).text_size(px(10.)))
                    .collect::<Vec<_>>(),
            ),
        )
        .into_any_element()
}

fn collect_canvases(node: &Value, path: &str, out: &mut HashMap<String, CanvasView>) {
    let Some(kind) = node["kind"].as_object().and_then(|m| m.keys().next()) else {
        return;
    };
    let v = &node["kind"][kind];
    if kind == "Canvas" {
        out.insert(
            path.into(),
            CanvasView {
                handler: v["draw"].as_u64().unwrap_or(0),
                height: number(&v["height"], 200.).clamp(40., 720.),
                animate: v["animate"] == true,
                bounds: Rc::new(Cell::new(Bounds::default())),
                pointer: Rc::new(Cell::new(None)),
                painted: Rc::new(Cell::new(None)),
                commands: Arc::new(vec![]),
                signature: None,
                requested: Instant::now(),
                pending: false,
                error: None,
            },
        );
    }
    for (i, n) in v["children"].as_array().into_iter().flatten().enumerate() {
        collect_canvases(n, &format!("{path}/{i}"), out);
    }
    for (r, row) in v["rows"].as_array().into_iter().flatten().enumerate() {
        for (c, n) in row.as_array().into_iter().flatten().enumerate() {
            collect_canvases(n, &format!("{path}/{r}/{c}"), out);
        }
    }
}
fn paint_commands(commands: &[Value], origin: Point<Pixels>, window: &mut Window, cx: &mut App) {
    for command in commands {
        if let Some(text) = command.get("Text") {
            let value = text["text"].as_str().unwrap_or_default();
            let font_size = px(text_size(&text["size"]));
            let line_height = font_size * 1.5;
            let mut font = gpui::font(if text["mono"] == true {
                "Geist Mono"
            } else {
                "Geist"
            });
            font.weight = weight(&text["weight"]);
            let run = TextRun {
                len: value.len(),
                font,
                color: color(text.get("color")),
                background_color: None,
                underline: None,
                strikethrough: None,
            };
            let line = window.text_system().shape_line(
                SharedString::from(value.to_owned()),
                font_size,
                &[run],
                None,
            );
            let mut at = origin + point(px(number(&text["x"], 0.)), px(number(&text["y"], 0.)));
            match text["align"].as_str() {
                Some("Center") => at.x -= line.width() / 2.,
                Some("End") => at.x -= line.width(),
                _ => {}
            }
            match text["baseline"].as_str() {
                Some("Center") => at.y -= line_height / 2.,
                Some("End") => at.y -= line_height,
                _ => {}
            }
            let _ = line.paint(at, line_height, TextAlign::Left, None, window, cx);
            continue;
        }
        let (v, stroke) = if let Some(v) = command.get("Stroke") {
            (v, true)
        } else if let Some(v) = command.get("Fill") {
            (v, false)
        } else {
            continue;
        };
        let mut path = if stroke {
            PathBuilder::stroke(px(number(&v["width"], 1.)))
        } else {
            PathBuilder::fill()
        };
        if stroke {
            if let Some(dash) = v["dash"].as_f64().filter(|d| *d > 0.) {
                path = path.dash_array(&[px(dash as f32), px(dash as f32)]);
            }
        }
        for (i, p) in v["points"].as_array().into_iter().flatten().enumerate() {
            let at = origin + point(px(number(&p[0], 0.)), px(number(&p[1], 0.)));
            if i == 0 {
                path.move_to(at);
            } else {
                path.line_to(at);
            }
        }
        if !stroke || v["closed"] == true {
            path.close();
        }
        if let Ok(path) = path.build() {
            window.paint_path(path, color(v.get("color")));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::prelude::v1::test;
    #[test]
    fn charts_preserve_stacking_negative_values_scatter_and_gaps() {
        let chart = json!({"kind":"Bar","stacked":true,"horizontal":true,"series":[{"values":[2,-3]},{"values":[5,-4]}]});
        let (marks, lo, hi) = chart_marks(&chart, 140., 100.);
        assert_eq!((lo, hi), (-7., 7.));
        assert_eq!(marks.len(), 4);
        let Mark::Box([x, _, width, _], _) = &marks[2] else {
            panic!("expected bar")
        };
        assert_eq!((*x, *width), (90., 50.));
        let (marks, _, _) = chart_marks(
            &json!({"kind":"Scatter","series":[{"points":[[10,2],[20,-4]]}]}),
            100.,
            100.,
        );
        assert!(matches!(&marks[0], Mark::Dot([0., _], _)));
        assert!(matches!(&marks[1], Mark::Dot([100., _], _)));
        let (marks, _, _) = chart_marks(
            &json!({"kind":"Line","series":[{"values":[1,null,3]}]}),
            100.,
            100.,
        );
        assert_eq!(marks.len(), 2);
    }
    #[gpui::test]
    fn reduced_canvases_and_keyboard_sliders_preserve_latest_intent(cx: &mut TestAppContext) {
        cx.update(initialize);
        let (requests, mut rx) = futures::channel::mpsc::unbounded();
        let (_, events) = futures::channel::mpsc::unbounded();
        let window = cx.add_window(|window, cx| {
            WorkspaceView::new(Transport { requests, events }, window, cx)
        });
        window.update(cx,|view,_,cx|{
            view.connected=true;view.selected=Some(json!({"id":"chat","deviceId":"host"}));
            view.open_visual("visual".into(),cx);let owner=view.visuals.views["visual"].owner;
            let slider=json!({"kind":{"Slider":{"label":"Scale","on_change":1,"value":2.0,"min":1.0,"max":5.0,"step":1.0}},"key":"scale"});
            view.receive(json!({"id":owner,"item":{"version":wire::VERSION,"lease":"owned","revision":0,"tree":slider}}),cx);
            let spec=SliderDrag{visual:"visual".into(),path:"".into(),min:1.,max:5.,step:1.};
            assert!(view.visual_slider_key(&spec,"right",cx));
            assert!(view.visual_slider_key(&spec,"right",cx));
            assert!(view.visual_slider_key(&spec,"left",cx));
            assert!(view.visual_slider_key(&spec,"left",cx));
            assert_eq!(view.visuals.views["visual"].queued_slider.as_ref().unwrap().1["value"],2.0);
            let mut updated=slider;updated["kind"]["Slider"]["value"]=json!(3.0);
            view.receive(json!({"id":owner,"item":{"version":wire::VERSION,"lease":"owned","revision":1,"tree":updated}}),cx);
            assert_eq!(view.visuals.views["visual"].slider_target.as_ref().unwrap().1,2.0);
            let canvas=json!({"kind":{"Canvas":{"draw":2,"height":100.0,"animate":true}}});
            view.receive(json!({"id":owner,"item":{"version":wire::VERSION,"lease":"owned","revision":2,"tree":canvas}}),cx);
            let c=view.visuals.views.get_mut("visual").unwrap().canvases.get_mut("").unwrap();
            c.bounds.set(Bounds::new(point(px(0.),px(0.)),size(px(200.),px(100.))));c.painted.set(Some(Instant::now()));
            c.signature=Some((200.,100.,None));c.requested=Instant::now()-Duration::from_secs(1);
            cx.set_reduce_motion(true);view.refresh_visual_draws(cx);assert!(!view.visuals.views["visual"].canvases[""].pending);
            view.environment.visible=false;cx.set_reduce_motion(false);view.refresh_visual_draws(cx);assert!(!view.visuals.views["visual"].canvases[""].pending);
            view.environment.visible=true;view.refresh_visual_draws(cx);assert!(view.visuals.views["visual"].canvases[""].pending);
            view.reset_visuals();
        }).unwrap();
        let mut controls = vec![];
        let mut draws = 0;
        while let Ok(frame) = rx.try_recv() {
            if frame["method"] == methods::CONTROL_VISUAL {
                controls.push(frame["params"]["argument"]["value"].clone());
            }
            if frame["method"] == methods::DRAW_VISUAL {
                draws += 1;
            }
        }
        assert_eq!(controls, vec![json!(3.0), json!(2.0)]);
        assert_eq!(draws, 1);
    }
    #[gpui::test]
    fn visual_controls_keep_host_revision_and_navigation_ownership(cx: &mut TestAppContext) {
        cx.update(initialize);
        let (requests, mut rx) = futures::channel::mpsc::unbounded();
        let (_, events) = futures::channel::mpsc::unbounded();
        let window = cx.add_window(|window, cx| {
            WorkspaceView::new(Transport { requests, events }, window, cx)
        });
        window.update(cx,|view,_,cx|{
            view.connected=true;view.device_id=Some("local".into());view.selected=Some(json!({"id":"chat","deviceId":"host"}));
            view.open_visual("visual".into(),cx);let owner=view.visuals.views["visual"].owner;
            let tree=json!({"kind":{"Button":{"label":"Increment","on_click":0,"primary":false}},"grow":false,"width":null,"key":null});
            view.receive(json!({"id":owner,"item":{"version":wire::VERSION,"lease":"owned","revision":0,"tree":tree}}),cx);
            view.visual_control("visual","",0,json!({"kind":"none"}),cx);
            view.visual_control("visual","",0,json!({"kind":"none"}),cx);
            assert!(view.visuals.views["visual"].busy);
            view.receive(json!({"id":owner,"item":{"version":wire::VERSION,"lease":"owned","revision":1,"tree":tree}}),cx);
            assert!(!view.visuals.views["visual"].busy);
            view.visual_control("visual","",0,json!({"kind":"none"}),cx); // obsolete rendered control
            let slider=json!({"kind":{"Slider":{"label":"Scale","on_change":1,"value":2.0,"min":1.0,"max":5.0,"step":1.0}},"key":"scale"});
            view.receive(json!({"id":owner,"item":{"version":wire::VERSION,"lease":"owned","revision":2,"tree":slider}}),cx);
            view.visual_control("visual","",2,json!({"kind":"number","value":2.0}),cx);
            view.visual_control("visual","",2,json!({"kind":"number","value":3.0}),cx);
            view.visual_control("visual","",2,json!({"kind":"number","value":3.0}),cx);
            let mut updated=slider;updated["kind"]["Slider"]["value"]=json!(3.0);
            view.receive(json!({"id":owner,"item":{"version":wire::VERSION,"lease":"owned","revision":3,"tree":updated}}),cx);
            assert!(!view.visuals.views["visual"].busy);
            assert!(view.visuals.views["visual"].queued_slider.is_none());
            view.reset_visuals();view.receive(json!({"id":owner,"item":{"version":wire::VERSION,"lease":"owned","revision":2,"tree":tree}}),cx);
            assert!(view.visuals.views.is_empty());
        }).unwrap();
        let mut controls = 0;
        while let Ok(frame) = rx.try_recv() {
            if frame["method"] == methods::CONTROL_VISUAL {
                controls += 1;
                assert_eq!(frame["params"]["targetDeviceId"], "host");
                assert_eq!(
                    frame["params"]["revision"],
                    if controls == 1 { 0 } else { 2 }
                );
            }
        }
        assert_eq!(controls, 2);
    }
}

fn same_slider(a: &Value, b: &Value) -> bool {
    input(a).is_some_and(|(_, kind)| kind == "Slider")
        && a["key"] == b["key"]
        && ["label", "min", "max", "step"]
            .iter()
            .all(|key| a["kind"]["Slider"][key] == b["kind"]["Slider"][key])
}
