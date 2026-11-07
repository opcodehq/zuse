//! Bar, line and donut charts painted with the ordered-dither engine.

use super::layout::{
    LINE_CHART_HEIGHT, PLOT_MARGINS, bar_spans, chart_value, compact_number, js_number,
    line_domain, nearest_index, percent_of, ticks, tooltip_left, x_label_step, x_pixel, y_pixel,
};
use super::{LineHover, UiBlockPalette, UiBlockView, eid, tabular};
use crate::dither::{
    DONUT_COLORS, DitherColor, DitherRaster, LineOpts, RasterImageCache, Rgb, backing_size,
    bar_fill, data_column, donut_fill, hover_column, line_band, paint_bands, paint_raster, stars,
};
use gpui::{prelude::*, *};
use std::cell::RefCell;
use std::rc::Rc;
use web_time::Instant;
use zuse_ui_block::ChartPoint;

/// Most x-axis labels a line chart draws.
const MAX_X_LABELS: usize = 4;
/// Grid lines (and y labels) requested from the tick generator.
const Y_TICKS: usize = 4;

fn rgb_color(rgb: Rgb) -> Hsla {
    Rgba {
        r: rgb[0] as f32 / 255.0,
        g: rgb[1] as f32 / 255.0,
        b: rgb[2] as f32 / 255.0,
        a: 1.0,
    }
    .into()
}

fn figure(p: &UiBlockPalette, gap: f32, title: &str) -> Div {
    div()
        .flex()
        .flex_col()
        .min_w_0()
        .gap(px(gap))
        .rounded(px(12.))
        .bg(p.muted.opacity(0.3))
        .p(px(16.))
        .child(
            div()
                .text_size(px(14.))
                .line_height(px(20.))
                .font_weight(FontWeight::MEDIUM)
                .child(title.to_string()),
        )
}

/// [`paint_raster`] with rounded corners, for fills that cover a rounded
/// track.
fn paint_rounded(
    bounds: Bounds<Pixels>,
    raster: &DitherRaster,
    cache: &mut RasterImageCache,
    radius: Pixels,
    window: &mut Window,
    cx: &mut App,
) {
    let scale = window.scale_factor();
    let width = (f32::from(bounds.size.width) * scale)
        .round()
        .clamp(1.0, 8192.0) as u32;
    let height = (f32::from(bounds.size.height) * scale)
        .round()
        .clamp(1.0, 8192.0) as u32;
    let image = cache.image(raster, (width, height), window, cx);
    let _ = window.paint_image(bounds, Corners::all(radius), image, 0, false);
}

fn mono_line(
    window: &mut Window,
    family: &SharedString,
    text: &str,
    size: f32,
    color: Hsla,
) -> ShapedLine {
    let run = TextRun {
        len: text.len(),
        font: font(family.clone()),
        color,
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    window
        .text_system()
        .shape_line(SharedString::from(text.to_string()), px(size), &[run], None)
}

impl UiBlockView {
    pub(super) fn bar_chart(
        &mut self,
        title: &str,
        points: &[ChartPoint],
        unit: Option<&str>,
        path: &str,
    ) -> AnyElement {
        let p = self.palette.clone();
        let values: Vec<f64> = points.iter().map(|point| point.value).collect();
        let (zero, spans) = bar_spans(&values);
        let green = DitherColor::Green.seed().fill;
        let caches: Vec<_> = (0..points.len())
            .map(|index| self.raster_cache(format!("{path}#bar{index}")))
            .collect();
        let rows = points.iter().zip(spans).zip(caches).map(|((point, span), cache)| {
            let track = div()
                .relative()
                .h(px(16.))
                .rounded(px(4.))
                .overflow_hidden()
                .bg(p.muted)
                .child(
                    div()
                        .absolute()
                        .top_0()
                        .bottom_0()
                        .w(px(1.))
                        .left(relative(zero as f32))
                        .bg(p.foreground.opacity(0.3)),
                )
                .child(
                    canvas(
                        |_, _, _| {},
                        move |bounds, _, window, cx| {
                            let (cols, rows) = backing_size(
                                f32::from(bounds.size.width) as f64,
                                f32::from(bounds.size.height) as f64,
                            );
                            let raster = bar_fill(cols, rows, span.start, span.fraction, green);
                            if raster.is_empty() {
                                return;
                            }
                            paint_rounded(
                                bounds,
                                &raster,
                                &mut cache.borrow_mut(),
                                px(4.),
                                window,
                                cx,
                            );
                        },
                    )
                    .absolute()
                    .inset_0(),
                );
            div()
                .flex()
                .flex_col()
                .gap(px(4.))
                .min_w_0()
                .child(
                    div()
                        .flex()
                        .justify_between()
                        .gap(px(12.))
                        .text_size(px(12.))
                        .line_height(px(16.))
                        .child(
                            div()
                                .min_w_0()
                                .text_color(p.muted_foreground)
                                .child(point.label.clone()),
                        )
                        .child(
                            div()
                                .flex_none()
                                .font_features(tabular())
                                .child(chart_value(point.value, unit)),
                        ),
                )
                .child(track)
        });
        figure(&p, 12.0, title)
            .child(div().flex().flex_col().gap(px(10.)).children(rows))
            .into_any_element()
    }

    pub(super) fn donut_chart(
        &mut self,
        title: &str,
        points: &[ChartPoint],
        unit: Option<&str>,
        path: &str,
    ) -> AnyElement {
        let p = self.palette.clone();
        let values: Vec<f64> = points.iter().map(|point| point.value).collect();
        let total: f64 = values.iter().sum();
        let cache = self.raster_cache(format!("{path}#donut"));
        let muted = p.muted;
        const BOX: f32 = 144.0;
        let ring = div()
            .relative()
            .flex_none()
            .size(px(BOX))
            .mx_auto()
            .child(
                canvas(
                    |_, _, _| {},
                    move |bounds, _, window, cx| {
                        if total == 0.0 {
                            // The empty track: r 50, stroke 12 in a 120 box.
                            let radius = f32::from(bounds.size.width) * 50.0 / 120.0;
                            let stroke = f32::from(bounds.size.width) * 12.0 / 120.0;
                            let outer = radius + stroke / 2.0;
                            window.paint_quad(quad(
                                Bounds::centered_at(
                                    bounds.center(),
                                    size(px(outer * 2.0), px(outer * 2.0)),
                                ),
                                px(outer),
                                transparent_black(),
                                px(stroke),
                                muted,
                                BorderStyle::default(),
                            ));
                            return;
                        }
                        let (cols, rows) = backing_size(
                            f32::from(bounds.size.width) as f64,
                            f32::from(bounds.size.height) as f64,
                        );
                        let raster = donut_fill(cols, rows, &values);
                        paint_raster(bounds, &raster, &mut cache.borrow_mut(), window, cx);
                    },
                )
                .absolute()
                .inset_0(),
            )
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .px(px(24.))
                    .text_center()
                    .child(
                        div()
                            .text_size(px(20.))
                            .line_height(px(28.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .font_features(tabular())
                            .child(compact_number(total)),
                    )
                    .when_some(unit, |d, unit| {
                        d.child(
                            div()
                                .text_size(px(12.))
                                .line_height(px(16.))
                                .text_color(p.muted_foreground)
                                .child(unit.to_string()),
                        )
                    }),
            );
        let legend = div()
            .flex()
            .flex_col()
            .flex_1()
            .flex_basis(px(160.))
            .min_w_0()
            .children(points.iter().enumerate().map(|(index, point)| {
                let swatch = DONUT_COLORS[index % DONUT_COLORS.len()].seed().fill;
                // One text size per row, so centring matches baseline alignment.
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .py(px(8.))
                    .text_size(px(12.))
                    .line_height(px(20.))
                    .when(index > 0, |d| {
                        d.border_t_1().border_color(p.border.opacity(0.4))
                    })
                    .child(
                        div()
                            .flex()
                            .flex_1()
                            .min_w_0()
                            .items_center()
                            .gap(px(8.))
                            .text_color(p.muted_foreground)
                            .child(
                                div()
                                    .flex_none()
                                    .size(px(8.))
                                    .rounded(px(4.))
                                    .bg(rgb_color(swatch)),
                            )
                            .child(div().min_w_0().child(point.label.clone())),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_none()
                            .text_right()
                            .font_features(tabular())
                            .child(chart_value(point.value, unit))
                            .child(
                                div()
                                    .ml(px(12.))
                                    .text_color(p.muted_foreground)
                                    .child(format!(" {}%", percent_of(point.value, total))),
                            ),
                    )
            }));
        figure(&p, 16.0, title)
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(20.))
                    .child(ring)
                    .child(legend),
            )
            .into_any_element()
    }

    pub(super) fn line_chart(
        &mut self,
        title: &str,
        points: &[ChartPoint],
        unit: Option<&str>,
        path: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = self.palette.clone();
        let values: Rc<Vec<f64>> = Rc::new(points.iter().map(|point| point.value).collect());
        let labels: Rc<Vec<String>> = Rc::new(points.iter().map(|point| point.label.clone()).collect());
        let len = values.len();
        let domain = line_domain(&values);
        let hover = self.line_hover.get(path).copied();
        let bounds_cell = self
            .chart_bounds
            .entry(path.to_string())
            .or_default()
            .clone();
        let cache: Rc<RefCell<RasterImageCache>> = self.raster_cache(format!("{path}#line"));
        let painted = self.line_painted.clone();
        let tick = self.tick;
        let reduce_motion = cx.reduce_motion();
        if !reduce_motion {
            self.ensure_ticker(cx);
        }
        let [margin_top, margin_right, margin_bottom, margin_left] = PLOT_MARGINS;
        let blue = DitherColor::Blue.seed().fill;

        let paint_values = values.clone();
        let paint_labels = labels.clone();
        let grid_color = p.border;
        let label_color = p.muted_foreground;
        let mono = p.font_mono.clone();
        let measured = bounds_cell.clone();
        let plot = canvas(
            move |bounds, _, _| measured.set(bounds),
            move |bounds, _, window, cx| {
                painted.set(Some(Instant::now()));
                let plot = Bounds::new(
                    point(
                        bounds.left() + px(margin_left),
                        bounds.top() + px(margin_top),
                    ),
                    size(
                        (bounds.size.width - px(margin_left + margin_right)).max(px(0.)),
                        (bounds.size.height - px(margin_top + margin_bottom)).max(px(0.)),
                    ),
                );
                let width = f32::from(plot.size.width) as f64;
                let height = f32::from(plot.size.height) as f64;
                let tick_values = ticks(domain.0, domain.1, Y_TICKS);
                for value in &tick_values {
                    let y = plot.top() + px(y_pixel(*value, domain, height) as f32);
                    let mut line = PathBuilder::stroke(px(1.)).dash_array(&[px(3.), px(3.)]);
                    line.move_to(point(plot.left(), y));
                    line.line_to(point(plot.right(), y));
                    if let Ok(line) = line.build() {
                        window.paint_path(line, grid_color);
                    }
                }

                let (cols, rows) = backing_size(width, height);
                let tops: Vec<f64> = paint_values
                    .iter()
                    .map(|value| {
                        y_pixel(*value, domain, height) / height.max(1.0) * (rows - 1) as f64
                    })
                    .collect();
                let band = line_band(&tops, cols, rows);
                let intensity = if hover.is_some() { 1.0 } else { 0.0 };
                let mut raster = DitherRaster::new(cols, rows);
                let bands = [(&band, blue)];
                paint_bands(
                    &mut raster,
                    &bands,
                    &LineOpts {
                        intensity,
                        ..LineOpts::default()
                    },
                );
                if let Some(hover) = hover {
                    let col = data_column(hover.index, len, cols);
                    if col >= 0 {
                        let top = band.top.get(col as usize).copied().unwrap_or(0.0);
                        hover_column(&mut raster, col, top, blue);
                    }
                }
                stars(&mut raster, &bands, len, tick, intensity, reduce_motion, 1.0);
                paint_raster(plot, &raster, &mut cache.borrow_mut(), window, cx);

                for value in &tick_values {
                    let line = mono_line(window, &mono, &js_number(*value), 10.0, label_color);
                    let y = plot.top() + px(y_pixel(*value, domain, height) as f32);
                    let origin = point(plot.left() - px(8.) - line.width, y - px(6.));
                    let _ = line.paint(origin, px(12.), TextAlign::Left, None, window, cx);
                }
                let step = x_label_step(len, MAX_X_LABELS);
                for index in (0..len).step_by(step) {
                    let line = mono_line(window, &mono, &paint_labels[index], 10.0, label_color);
                    let x = plot.left() + px(x_pixel(index, len, width) as f32);
                    let origin = point(x - line.width / 2.0, plot.bottom() + px(8.));
                    let _ = line.paint(origin, px(12.), TextAlign::Left, None, window, cx);
                }
            },
        )
        .absolute()
        .inset_0();

        let tooltip = hover.and_then(|hover| {
            let value = *values.get(hover.index)?;
            let heading = labels.get(hover.index)?.clone();
            let shown = chart_value(value, unit);
            let mono = p.font_mono.clone();
            let heading_width = mono_line(window, &mono, &heading, 10.0, p.muted_foreground).width;
            let label_width = mono_line(window, &mono, title, 11.0, p.muted_foreground).width;
            let value_width = mono_line(window, &mono, &shown, 11.0, p.foreground).width;
            let content = f32::from(heading_width)
                .max(8.0 + 6.0 + f32::from(label_width) + 8.0 + f32::from(value_width));
            let width = content.ceil() + 1.0 + 16.0 + 2.0;
            let height = 2.0 + 8.0 + 15.0 + 2.0 + 16.5;
            let chart = bounds_cell.get();
            let plot_width = f32::from(chart.size.width) - margin_left - margin_right;
            let left = tooltip_left(hover.cursor_x, plot_width, margin_left);
            let plot_height = (LINE_CHART_HEIGHT - margin_top - margin_bottom) as f64;
            let top = (margin_top + 44.0)
                .max(margin_top + y_pixel(value, domain, plot_height) as f32);
            Some(
                div()
                    .absolute()
                    .left(px(left - width / 2.0))
                    .top(px(top - height * 1.15))
                    .w(px(width))
                    .flex()
                    .flex_col()
                    .rounded(px(7.))
                    .border_1()
                    .border_color(p.border)
                    .bg(p.popover)
                    .shadow_sm()
                    .px(px(8.))
                    .py(px(4.))
                    .font_family(mono)
                    .whitespace_nowrap()
                    .child(
                        div()
                            .mb(px(2.))
                            .text_size(px(10.))
                            .line_height(px(15.))
                            .text_color(p.muted_foreground)
                            .child(heading),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .text_size(px(11.))
                            .line_height(px(16.5))
                            .font_features(tabular())
                            .child(
                                div()
                                    .flex_none()
                                    .size(px(8.))
                                    .rounded(px(1.))
                                    .bg(rgb_color(blue)),
                            )
                            .child(
                                div()
                                    .text_color(p.muted_foreground)
                                    .child(title.to_string()),
                            )
                            .child(
                                div()
                                    .ml_auto()
                                    .pl(px(8.))
                                    .text_color(p.foreground)
                                    .child(shown),
                            ),
                    ),
            )
        });

        let key = path.to_string();
        let leave_key = path.to_string();
        let pointer_bounds = bounds_cell;
        let area = div()
            .id(eid(path, "line"))
            .relative()
            .w_full()
            .h(px(LINE_CHART_HEIGHT))
            .role(Role::Image)
            .aria_label(title.to_string())
            .on_mouse_move(cx.listener(move |this, event: &MouseMoveEvent, _, cx| {
                let bounds = pointer_bounds.get();
                let x = f32::from(event.position.x - bounds.left());
                let plot_width = f32::from(bounds.size.width) - margin_left - margin_right;
                let next = LineHover {
                    index: nearest_index((x - margin_left) as f64, len, plot_width as f64),
                    cursor_x: x,
                };
                if this.line_hover.get(&key) != Some(&next) {
                    this.line_hover.insert(key.clone(), next);
                    cx.notify();
                }
            }))
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                if !*hovered && this.line_hover.remove(&leave_key).is_some() {
                    cx.notify();
                }
            }))
            .child(plot)
            .children(tooltip);
        figure(&p, 8.0, title).child(area).into_any_element()
    }
}
