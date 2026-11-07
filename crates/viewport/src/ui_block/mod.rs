//! Native renderer for UI blocks: agent-authored component specs parsed by
//! `zuse_ui_block` and painted inline in chat, with charts drawn by the
//! ordered-dither engine in [`crate::dither`].
//!
//! [`UiBlockView`] owns all local state (tab selection, form values, chart
//! hover). A submitted Form or a chosen follow-up emits
//! [`UiBlockEvent::Send`]; the host routes the text to the composer and calls
//! [`UiBlockView::mark_sent`] to lock the block.

mod charts;
mod demos;
mod form;
pub mod layout;
mod palette;
#[cfg(test)]
mod tests;

use crate::dither::RasterImageCache;
use gpui::{prelude::*, *};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;
use web_time::Instant;
use zuse_ui_block::{Component, FieldValue, StepStatus, Tone};

pub use demos::demo_specs;
pub use palette::UiBlockPalette;

use layout::{
    FLOW_ROW, GRID_GAP, GRID_MIN_COMPACT, GRID_MIN_WIDE, NarrowLine, TABLE_NARROW, column_weights,
    grid_columns, narrow_lines, progress_percent,
};

/// What a UI block asks its host to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UiBlockEvent {
    /// Send `text` as the user's next message (a Form answer or follow-up).
    Send { text: String },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Status {
    Idle,
    /// A send was requested; controls lock until the host settles it.
    Sending,
    Sent,
}

/// Hover state of one line chart.
#[derive(Clone, Copy, Debug, PartialEq)]
struct LineHover {
    index: usize,
    /// Pointer x relative to the chart box.
    cursor_x: f32,
}

/// A rendered UI block. Create one per block with [`UiBlockView::new`].
pub struct UiBlockView {
    source: String,
    root: Result<Rc<Component>, SharedString>,
    palette: Rc<UiBlockPalette>,
    interactive: bool,
    padded: bool,
    status: Status,
    /// Field values per Form path.
    forms: HashMap<String, Vec<(String, FieldValue)>>,
    text_fields: HashMap<String, form::TextField>,
    tabs: HashMap<String, usize>,
    open_select: Option<String>,
    chosen_follow_up: Option<(String, usize)>,
    /// Measured container widths per responsive component path.
    widths: HashMap<String, Rc<Cell<f32>>>,
    rasters: HashMap<String, Rc<RefCell<RasterImageCache>>>,
    chart_bounds: HashMap<String, Rc<Cell<Bounds<Pixels>>>>,
    line_hover: HashMap<String, LineHover>,
    /// Last time a line chart painted, so stars only twinkle while visible.
    line_painted: Rc<Cell<Option<Instant>>>,
    tick: u64,
    ticking: bool,
    _ticker: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<UiBlockEvent> for UiBlockView {}

impl UiBlockView {
    /// Parse `source` and render it; an invalid spec renders the fallback
    /// with the raw source. `interactive` enables forms and follow-ups.
    pub fn new(
        source: &str,
        palette: UiBlockPalette,
        interactive: bool,
        cx: &mut Context<Self>,
    ) -> Self {
        let root = zuse_ui_block::parse(source)
            .map(|spec| Rc::new(spec.root))
            .map_err(|error| SharedString::from(error.to_string()));
        cx.on_release(|this: &mut Self, cx| {
            for cache in this.rasters.values() {
                cache.borrow_mut().release(cx);
            }
        })
        .detach();
        Self {
            source: source.to_string(),
            root,
            palette: Rc::new(palette),
            interactive,
            padded: true,
            status: Status::Idle,
            forms: HashMap::new(),
            text_fields: HashMap::new(),
            tabs: HashMap::new(),
            open_select: None,
            chosen_follow_up: None,
            widths: HashMap::new(),
            rasters: HashMap::new(),
            chart_bounds: HashMap::new(),
            line_hover: HashMap::new(),
            line_painted: Rc::new(Cell::new(None)),
            tick: 0,
            ticking: false,
            _ticker: None,
            _subscriptions: Vec::new(),
        }
    }

    /// The spec source this view was created with.
    pub fn source(&self) -> &str {
        &self.source
    }

    /// Whether the spec parsed (false renders the fallback).
    pub fn is_valid(&self) -> bool {
        self.root.is_ok()
    }

    pub fn set_palette(&mut self, palette: UiBlockPalette, cx: &mut Context<Self>) {
        if *self.palette == palette {
            return;
        }
        self.palette = Rc::new(palette);
        let style = form::editor_style(&self.palette);
        for field in self.text_fields.values() {
            field.set_style(style.clone(), cx);
        }
        cx.notify();
    }

    /// Enable or disable forms and follow-ups (the host enables only the
    /// latest turn).
    pub fn set_interactive(&mut self, interactive: bool, cx: &mut Context<Self>) {
        if self.interactive != interactive {
            self.interactive = interactive;
            if !interactive {
                self.open_select = None;
            }
            cx.notify();
        }
    }

    /// Drop the block's own 12×6 gutter when the host already supplies one.
    pub fn set_padded(&mut self, padded: bool, cx: &mut Context<Self>) {
        if self.padded != padded {
            self.padded = padded;
            cx.notify();
        }
    }

    /// The host accepted the sent answer: show "Sent" and keep every control
    /// locked.
    pub fn mark_sent(&mut self, cx: &mut Context<Self>) {
        self.status = Status::Sent;
        self.open_select = None;
        cx.notify();
    }

    /// The host could not send the answer: unlock so the user can retry.
    pub fn mark_send_failed(&mut self, cx: &mut Context<Self>) {
        if self.status == Status::Sending {
            self.status = Status::Idle;
            self.chosen_follow_up = None;
            cx.notify();
        }
    }

    fn disabled(&self) -> bool {
        !self.interactive || self.status != Status::Idle
    }

    fn send(&mut self, text: String, cx: &mut Context<Self>) {
        if self.disabled() {
            return;
        }
        self.status = Status::Sending;
        self.open_select = None;
        cx.emit(UiBlockEvent::Send { text });
        cx.notify();
    }

    /// The measured width of the container at `path` (0 until first layout)
    /// and the cell its probe writes to.
    fn width_of(&mut self, path: &str) -> (f32, Rc<Cell<f32>>) {
        let cell = self
            .widths
            .entry(path.to_string())
            .or_insert_with(|| Rc::new(Cell::new(0.0)))
            .clone();
        (cell.get(), cell)
    }

    fn raster_cache(&mut self, key: String) -> Rc<RefCell<RasterImageCache>> {
        self.rasters.entry(key).or_default().clone()
    }

    fn node(
        &mut self,
        node: &Component,
        path: &str,
        form: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = self.palette.clone();
        match node {
            Component::Card { children, title } => div()
                .flex()
                .flex_col()
                .min_w_0()
                .gap(px(16.))
                .when_some(title.as_ref(), |d, title| d.child(heading(&p, title)))
                .children(self.children(children, path, form, window, cx))
                .into_any_element(),
            Component::Text { text } => text_block(&p, text),
            Component::Stat { label, value, hint } => stat(&p, label, value, hint.as_deref()),
            Component::KeyValue { pairs } => key_value(&p, pairs),
            Component::Table { columns, rows } => self.table(columns, rows, path, cx),
            Component::Progress { label, percent } => progress(&p, label, *percent),
            Component::Badge { label, tone } => badge(&p, label, tone.unwrap_or(Tone::Neutral)),
            Component::List { items } => list(&p, items),
            Component::Grid { children, columns } => {
                self.grid(children, columns.unwrap_or(2), path, form, window, cx)
            }
            Component::BarChart {
                title,
                points,
                unit,
            } => self.bar_chart(title, points, unit.as_deref(), path),
            Component::LineChart {
                title,
                points,
                unit,
            } => self.line_chart(title, points, unit.as_deref(), path, window, cx),
            Component::DonutChart {
                title,
                points,
                unit,
            } => self.donut_chart(title, points, unit.as_deref(), path),
            Component::Callout { text, tone, title } => {
                callout(&p, text, tone.unwrap_or(Tone::Neutral), title.as_deref())
            }
            Component::Steps { items } => steps(&p, items, path, cx),
            Component::Flow { nodes, caption } => self.flow(nodes, caption.as_deref(), path, cx),
            Component::Tabs { tabs } => self.tabs(tabs, path, form, window, cx),
            Component::FollowUps { items } => self.follow_ups(items, path, cx),
            Component::Form { .. } => self.form(node, path, window, cx),
            Component::Input { .. }
            | Component::TextArea { .. }
            | Component::Select { .. }
            | Component::RadioGroup { .. }
            | Component::Checkbox { .. }
            | Component::Slider { .. } => self.field(node, path, form, window, cx),
        }
    }

    fn children(
        &mut self,
        children: &[Component],
        path: &str,
        form: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        children
            .iter()
            .enumerate()
            .map(|(index, child)| self.node(child, &format!("{path}.{index}"), form, window, cx))
            .collect()
    }

    fn table(
        &mut self,
        columns: &[String],
        rows: &[Vec<String>],
        path: &str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = self.palette.clone();
        let (width, cell) = self.width_of(path);
        let narrow = width > 0.0 && width < TABLE_NARROW;
        let divider = p.border.opacity(0.4);
        let last = rows.len().saturating_sub(1);
        let body_cell = |first: bool| {
            div()
                .min_w_0()
                .min_h(px(28.))
                .px(px(12.))
                .py(px(8.))
                .text_size(px(12.))
                .line_height(px(20.))
                .font_features(tabular())
                .when(first, |d| {
                    d.font_weight(FontWeight::MEDIUM).text_color(p.foreground)
                })
        };
        let table = div()
            .flex()
            .flex_col()
            .min_w_0()
            .rounded(px(12.))
            .overflow_hidden()
            .bg(p.muted.opacity(0.2))
            .text_size(px(12.));
        let table = if narrow {
            table.children(rows.iter().enumerate().map(|(row_index, row)| {
                let lines = narrow_lines(row);
                let cell_at = |index: usize| {
                    let text = row.get(index).map_or("", String::as_str);
                    body_cell(index == 0)
                        .child(
                            div()
                                .text_size(px(11.))
                                .font_weight(FontWeight::NORMAL)
                                .text_color(p.muted_foreground)
                                .child(columns.get(index).cloned().unwrap_or_default()),
                        )
                        .child(text.to_string())
                };
                div()
                    .id(eid(path, &format!("row{row_index}")))
                    .flex()
                    .flex_col()
                    .py(px(8.))
                    .when(row_index != last, |d| d.border_b_1().border_color(divider))
                    .when(row_index == last, |d| d.rounded_b(px(12.)))
                    .when(row_index == 0, |d| d.rounded_t(px(12.)))
                    .hover(|style| style.bg(p.row_hover()))
                    .children(lines.into_iter().map(|line| {
                        match line {
                            NarrowLine::Full(index) => div().flex().child(cell_at(index).w_full()),
                            NarrowLine::Pair(left, right) => div()
                                .flex()
                                .child(cell_at(left).w(relative(0.5)))
                                .when_some(right, |d, right| {
                                    d.child(cell_at(right).w(relative(0.5)))
                                }),
                        }
                    }))
            }))
        } else {
            let weights = column_weights(columns, rows);
            let total: f32 = weights.iter().sum::<f32>().max(1.0);
            let header = div()
                .flex()
                .rounded_t(px(12.))
                .bg(p.muted.opacity(0.4))
                .border_b_1()
                .border_color(divider)
                .children(columns.iter().enumerate().map(|(index, column)| {
                    div()
                        .flex_none()
                        .w(relative(weights[index] / total))
                        .min_h(px(28.))
                        .px(px(12.))
                        .py(px(8.))
                        .text_size(px(11.))
                        .line_height(px(16.))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(p.muted_foreground)
                        .child(column.clone())
                }));
            table
                .child(header)
                .children(rows.iter().enumerate().map(|(row_index, row)| {
                    div()
                        .id(eid(path, &format!("row{row_index}")))
                        .flex()
                        .when(row_index != last, |d| d.border_b_1().border_color(divider))
                        .when(row_index == last, |d| d.rounded_b(px(12.)))
                        .hover(|style| style.bg(p.row_hover()))
                        .children(row.iter().enumerate().map(|(index, text)| {
                            body_cell(index == 0)
                                .child(text.clone())
                                .flex_none()
                                .w(relative(
                                    weights.get(index).copied().unwrap_or(12.0) / total,
                                ))
                        }))
                }))
        };
        div()
            .relative()
            .min_w_0()
            .child(width_probe(cell, cx.entity_id()))
            .child(table)
            .into_any_element()
    }

    fn grid(
        &mut self,
        children: &[Component],
        columns: u8,
        path: &str,
        form: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let compact = children.iter().all(|child| {
            matches!(
                child,
                Component::Stat { .. } | Component::Badge { .. } | Component::Progress { .. }
            )
        });
        let minimum = if compact {
            GRID_MIN_COMPACT
        } else {
            GRID_MIN_WIDE
        };
        let (width, cell) = self.width_of(path);
        let tracks = grid_columns(width, columns, minimum, children.len());
        let items = self.children(children, path, form, window, cx);
        div()
            .relative()
            .min_w_0()
            .child(width_probe(cell, cx.entity_id()))
            .child(
                div()
                    .grid()
                    .grid_cols(tracks as u16)
                    .gap(px(GRID_GAP))
                    .min_w_0()
                    .children(items),
            )
            .into_any_element()
    }

    fn flow(
        &mut self,
        nodes: &[zuse_ui_block::FlowNode],
        caption: Option<&str>,
        path: &str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = self.palette.clone();
        let (width, cell) = self.width_of(path);
        let row = width <= 0.0 || width >= FLOW_ROW;
        let mut diagram = div().flex().gap(px(6.)).min_w_0();
        diagram = if row {
            diagram.flex_row().items_center()
        } else {
            diagram.flex_col()
        };
        for (index, node) in nodes.iter().enumerate() {
            if index > 0 {
                let arrow = icon("arrow-right", 16.0, p.muted_foreground).self_center();
                diagram = diagram.child(if row {
                    arrow
                } else {
                    arrow.with_transformation(Transformation::rotate(percentage(0.25)))
                });
            }
            let highlight = node.highlight;
            diagram = diagram.child(
                div()
                    .flex()
                    .flex_col()
                    .justify_center()
                    .min_w_0()
                    .when(row, |d| d.flex_1().self_stretch())
                    .rounded(px(10.))
                    .px(px(12.))
                    .py(px(8.))
                    .text_center()
                    .bg(p.muted.opacity(0.5))
                    .when(highlight, |d| {
                        d.bg(p.primary.opacity(0.15))
                            .border_1()
                            .border_color(p.primary.opacity(0.4))
                            .px(px(11.))
                            .py(px(7.))
                    })
                    .child(
                        div()
                            .text_size(px(14.))
                            .line_height(px(20.))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(p.foreground)
                            .child(node.label.clone()),
                    )
                    .when_some(node.detail.as_ref(), |d, detail| {
                        d.child(
                            div()
                                .text_size(px(12.))
                                .line_height(px(20.))
                                .text_color(p.muted_foreground)
                                .child(detail.clone()),
                        )
                    }),
            );
        }
        div()
            .relative()
            .flex()
            .flex_col()
            .gap(px(8.))
            .min_w_0()
            .child(width_probe(cell, cx.entity_id()))
            .child(diagram)
            .when_some(caption, |d, caption| {
                d.child(
                    div()
                        .text_size(px(12.))
                        .line_height(px(16.))
                        .text_color(p.muted_foreground)
                        .child(caption.to_string()),
                )
            })
            .into_any_element()
    }

    fn tabs(
        &mut self,
        tabs: &[zuse_ui_block::Tab],
        path: &str,
        form: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = self.palette.clone();
        let active = self
            .tabs
            .get(path)
            .copied()
            .unwrap_or(0)
            .min(tabs.len().saturating_sub(1));
        let bar = div()
            .flex()
            .items_center()
            .h(px(28.))
            .min_w_0()
            .p(px(2.))
            .gap(px(2.))
            .rounded(px(7.))
            .bg(p.muted.opacity(0.55))
            .id(eid(path, "tabs"))
            .role(Role::TabList)
            .children(tabs.iter().enumerate().map(|(index, tab)| {
                let selected = index == active;
                let key = path.to_string();
                div()
                    .id(eid(path, &format!("tab{index}")))
                    .role(Role::Tab)
                    .aria_selected(selected)
                    .aria_label(tab.label.clone())
                    .tab_index(0)
                    .flex()
                    .items_center()
                    .justify_center()
                    .min_w_0()
                    .h(px(24.))
                    .px(px(10.))
                    .rounded(px(5.))
                    .text_size(px(11.))
                    .line_height(px(16.))
                    .font_weight(FontWeight::MEDIUM)
                    .cursor_pointer()
                    .map(|d| {
                        if selected {
                            d.bg(p.background).text_color(p.foreground)
                        } else {
                            let hover = p.background.opacity(0.45);
                            let fg = p.foreground;
                            d.text_color(p.muted_foreground)
                                .hover(move |style| style.bg(hover).text_color(fg))
                        }
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.tabs.insert(key.clone(), index);
                        cx.notify();
                    }))
                    .child(div().truncate().child(tab.label.clone()))
            }));
        let panel = tabs
            .get(active)
            .map(|tab| self.node(&tab.content, &format!("{path}.{active}"), form, window, cx));
        div()
            .flex()
            .flex_col()
            .gap(px(12.))
            .min_w_0()
            .child(div().flex().child(bar))
            .child(div().min_w_0().flex().flex_col().children(panel))
            .into_any_element()
    }

    fn fallback(&self, reason: &str, cx: &mut Context<Self>) -> AnyElement {
        let p = &self.palette;
        let title = crate::localization::message(cx, |l| {
            zuse_i18n::messages::chat::message_row_ui_spec_could_not_render(l)
        });
        div()
            .flex()
            .flex_col()
            .min_w_0()
            .rounded(px(10.))
            .border_1()
            .border_dashed()
            .border_color(p.border.opacity(0.5))
            .bg(p.muted.opacity(0.3))
            .px(px(12.))
            .py(px(8.))
            .child(
                div()
                    .text_size(px(11.))
                    .line_height(px(16.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(p.muted_foreground)
                    .child(title),
            )
            .when(!reason.is_empty(), |d| {
                d.child(
                    div()
                        .mt(px(2.))
                        .text_size(px(11.))
                        .line_height(px(16.))
                        .text_color(p.muted_foreground.opacity(0.8))
                        .child(reason.to_string()),
                )
            })
            .child(
                div()
                    .id("ui-block-source")
                    .mt(px(6.))
                    .max_h(px(192.))
                    .overflow_y_scroll()
                    .font_family(p.font_mono.clone())
                    .text_size(px(11.))
                    .line_height(px(16.))
                    .text_color(p.muted_foreground.opacity(0.8))
                    .child(self.source.clone()),
            )
            .into_any_element()
    }

    /// Keep stars twinkling on a ~100ms tick while a line chart is visible.
    fn ensure_ticker(&mut self, cx: &mut Context<Self>) {
        if self.ticking || cx.reduce_motion() {
            return;
        }
        self.ticking = true;
        self._ticker = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(100))
                    .await;
                let keep = this
                    .update(cx, |this, cx| {
                        let visible = this
                            .line_painted
                            .get()
                            .is_some_and(|at| at.elapsed() < Duration::from_millis(400));
                        if visible && !cx.reduce_motion() {
                            this.tick = this.tick.wrapping_add(1);
                            cx.notify();
                            true
                        } else {
                            this.ticking = false;
                            false
                        }
                    })
                    .unwrap_or(false);
                if !keep {
                    break;
                }
            }
        }));
    }
}

impl Render for UiBlockView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = self.palette.clone();
        let body = match self.root.clone() {
            Ok(root) => self.node(&root, "r", None, window, cx),
            Err(reason) => self.fallback(&reason, cx),
        };
        div()
            .flex()
            .flex_col()
            .w_full()
            .min_w_0()
            .when(self.padded, |d| d.px(px(12.)).py(px(6.)))
            .font_family(p.font_sans.clone())
            .text_color(p.foreground)
            .text_size(px(14.))
            .line_height(px(20.))
            .id("ui-block")
            .role(Role::Group)
            .aria_label(crate::localization::message(cx, |l| {
                zuse_i18n::messages::chat::message_row_ui_spec_generated(l)
            }))
            .child(body)
    }
}

// ---------------------------------------------------------------------------
// Shared pieces
// ---------------------------------------------------------------------------

fn eid(path: &str, part: &str) -> ElementId {
    ElementId::Name(SharedString::from(format!("ui-block-{path}-{part}")))
}

fn tabular() -> FontFeatures {
    FontFeatures(Arc::new(vec![("tnum".into(), 1)]))
}

fn icon(name: &str, size: f32, color: Hsla) -> Svg {
    svg()
        .path(SharedString::from(format!("icons/{name}.svg")))
        .size(px(size))
        .flex_none()
        .text_color(color)
}

/// An invisible layer that records its container's width and re-renders
/// the view when it changes, for container-width breakpoints.
fn width_probe(cell: Rc<Cell<f32>>, entity: EntityId) -> impl IntoElement {
    canvas(
        move |bounds, _, cx| {
            let width = f32::from(bounds.size.width);
            if (cell.get() - width).abs() >= 0.5 {
                cell.set(width);
                cx.notify(entity);
            }
        },
        |_, _, _, _| {},
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
}

fn heading(p: &UiBlockPalette, title: &str) -> Div {
    div()
        .text_size(px(14.))
        .line_height(px(20.))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(p.foreground)
        .child(title.to_string())
}

fn text_block(p: &UiBlockPalette, text: &str) -> AnyElement {
    div()
        .min_w_0()
        .text_size(px(14.))
        .line_height(px(24.))
        .text_color(p.foreground.opacity(0.9))
        .child(text.to_string())
        .into_any_element()
}

fn stat(p: &UiBlockPalette, label: &str, value: &str, hint: Option<&str>) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .min_w_0()
        .gap(px(4.))
        .rounded(px(12.))
        .bg(p.muted.opacity(0.4))
        .px(px(16.))
        .py(px(12.))
        .child(
            div()
                .text_size(px(12.))
                .line_height(px(16.))
                .text_color(p.muted_foreground)
                .child(label.to_string()),
        )
        .child(
            div()
                .text_size(px(24.))
                .line_height(px(32.))
                .font_weight(FontWeight::SEMIBOLD)
                .font_features(tabular())
                .text_color(p.foreground)
                .child(value.to_string()),
        )
        .when_some(hint, |d, hint| {
            d.child(
                div()
                    .text_size(px(11.))
                    .line_height(px(16.))
                    .text_color(p.muted_foreground.opacity(0.7))
                    .child(hint.to_string()),
            )
        })
        .into_any_element()
}

fn key_value(p: &UiBlockPalette, pairs: &[zuse_ui_block::KeyValuePair]) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .min_w_0()
        .rounded(px(12.))
        .bg(p.muted.opacity(0.25))
        .px(px(12.))
        .children(pairs.iter().enumerate().map(|(index, pair)| {
            div()
                .flex()
                .items_baseline()
                .gap(px(12.))
                .py(px(8.))
                .text_size(px(12.))
                .line_height(px(20.))
                .when(index > 0, |d| {
                    d.border_t_1().border_color(p.border.opacity(0.4))
                })
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_color(p.muted_foreground)
                        .child(pair.key.clone()),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_right()
                        .font_features(tabular())
                        .text_color(p.foreground.opacity(0.9))
                        .child(pair.value.clone()),
                )
        }))
        .into_any_element()
}

fn progress(p: &UiBlockPalette, label: &str, percent: f64) -> AnyElement {
    let clamped = percent.clamp(0.0, 100.0);
    div()
        .flex()
        .flex_col()
        .min_w_0()
        .gap(px(6.))
        .child(
            div()
                .flex()
                .items_baseline()
                .justify_between()
                .gap(px(8.))
                .text_size(px(14.))
                .line_height(px(20.))
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_color(p.foreground.opacity(0.9))
                        .child(label.to_string()),
                )
                .child(
                    div()
                        .flex_none()
                        .text_size(px(12.))
                        .line_height(px(16.))
                        .font_features(tabular())
                        .text_color(p.muted_foreground)
                        .child(format!("{}%", progress_percent(percent))),
                ),
        )
        .child(
            div()
                .h(px(6.))
                .rounded_full()
                .overflow_hidden()
                .bg(p.muted)
                .child(
                    div()
                        .h_full()
                        .w(relative((clamped / 100.0) as f32))
                        .rounded_full()
                        .bg(p.primary),
                ),
        )
        .into_any_element()
}

fn badge(p: &UiBlockPalette, label: &str, tone: Tone) -> AnyElement {
    let (bg, fg) = match tone {
        Tone::Neutral => (p.muted.opacity(0.5), p.foreground),
        Tone::Good => (p.alert_success_bg, p.success),
        Tone::Warn => (p.alert_warning_bg, p.warning),
        Tone::Bad => (p.alert_error_bg, p.destructive),
    };
    // The wrapper keeps the pill at its intrinsic width inside grids and
    // columns (`self-start`).
    div()
        .flex()
        .items_start()
        .min_w_0()
        .child(
            div()
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .h(px(18.))
                .min_w(px(18.))
                .px(px(4.))
                .rounded_full()
                .bg(bg)
                .text_color(fg)
                .text_size(px(10.))
                .line_height(px(18.))
                .font_weight(FontWeight::MEDIUM)
                .whitespace_nowrap()
                .child(label.to_string()),
        )
        .into_any_element()
}

fn list(p: &UiBlockPalette, items: &[String]) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .min_w_0()
        .gap(px(4.))
        .pl(px(20.))
        .text_size(px(14.))
        .line_height(px(24.))
        .text_color(p.foreground.opacity(0.9))
        .children(items.iter().map(|item| {
            div()
                .relative()
                .min_w_0()
                .child(
                    div()
                        .absolute()
                        .left(px(-13.))
                        .top(px(10.))
                        .size(px(5.))
                        .rounded_full()
                        .bg(p.muted_foreground),
                )
                .child(item.clone())
        }))
        .into_any_element()
}

fn callout(p: &UiBlockPalette, text: &str, tone: Tone, title: Option<&str>) -> AnyElement {
    let (border, bg) = match tone {
        Tone::Neutral => (p.border, p.muted.opacity(0.3)),
        Tone::Good => (p.success, p.success.opacity(0.05)),
        Tone::Warn => (p.warning, p.warning.opacity(0.05)),
        Tone::Bad => (p.destructive, p.destructive.opacity(0.05)),
    };
    div()
        .flex()
        .flex_col()
        .min_w_0()
        .border_l_2()
        .border_color(border)
        .rounded_r(px(10.))
        .bg(bg)
        .px(px(12.))
        .py(px(10.))
        .text_size(px(12.))
        .line_height(px(20.))
        .when_some(title, |d, title| {
            d.child(
                div()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(p.foreground)
                    .child(title.to_string()),
            )
        })
        .child(
            div()
                .min_w_0()
                .text_color(p.foreground.opacity(0.8))
                .child(text.to_string()),
        )
        .into_any_element()
}

fn steps(
    p: &UiBlockPalette,
    items: &[zuse_ui_block::Step],
    path: &str,
    cx: &mut Context<UiBlockView>,
) -> AnyElement {
    let reduce_motion = cx.reduce_motion();
    div()
        .flex()
        .flex_col()
        .min_w_0()
        .gap(px(12.))
        .rounded(px(12.))
        .bg(p.muted.opacity(0.25))
        .p(px(12.))
        .text_size(px(14.))
        .children(items.iter().enumerate().map(|(index, item)| {
            let marker: AnyElement = match item.status {
                StepStatus::Done => icon("check", 14.0, p.success).into_any_element(),
                StepStatus::Failed => icon("close", 14.0, p.destructive).into_any_element(),
                StepStatus::Pending => div()
                    .size(px(10.))
                    .rounded_full()
                    .border_1()
                    .border_color(p.muted_foreground.opacity(0.6))
                    .into_any_element(),
                StepStatus::Active => {
                    let dot = div().size(px(8.)).rounded_full().bg(p.primary);
                    if reduce_motion {
                        dot.into_any_element()
                    } else {
                        dot.with_animation(
                            eid(path, &format!("pulse{index}")),
                            Animation::new(Duration::from_secs(2))
                                .repeat()
                                .with_easing(pulsating_between(0.5, 1.0)),
                            |dot, delta| dot.opacity(delta),
                        )
                        .into_any_element()
                    }
                }
            };
            let label_color = match item.status {
                StepStatus::Pending => p.muted_foreground,
                StepStatus::Active => p.foreground,
                _ => p.foreground.opacity(0.9),
            };
            div()
                .flex()
                .gap(px(10.))
                .min_w_0()
                .child(
                    div()
                        .mt(px(4.))
                        .size(px(16.))
                        .flex_none()
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(marker),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_w_0()
                        .child(
                            div()
                                .line_height(px(24.))
                                .text_color(label_color)
                                .when(item.status == StepStatus::Active, |d| {
                                    d.font_weight(FontWeight::MEDIUM)
                                })
                                .child(item.label.clone()),
                        )
                        .when_some(item.detail.as_ref(), |d, detail| {
                            d.child(
                                div()
                                    .text_size(px(12.))
                                    .line_height(px(20.))
                                    .text_color(p.muted_foreground)
                                    .child(detail.clone()),
                            )
                        }),
                )
        }))
        .into_any_element()
}
