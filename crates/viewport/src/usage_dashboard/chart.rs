//! Dithered series share the reference plot's margins, axes and hover/day selection.
use super::view::{column, label, money, tokens};
use super::*;
use crate::palette::rgb;
use crate::shell::{BORDER, MUTED};
use std::{cell::Cell, rc::Rc, sync::Arc};
use zuse_protocol::usage::Group;
impl UsageDashboard {
    pub(super) fn chart(&self, groups: &[Group], cx: &mut Context<Self>) -> AnyElement {
        let groups = groups
            .iter()
            .rev()
            .take(90)
            .cloned()
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<Vec<_>>();
        let cost = self.cost;
        let values: Vec<Vec<f64>> = groups
            .iter()
            .map(|g| {
                if cost {
                    vec![g.summary.cost_usd.unwrap_or(0.)]
                } else {
                    vec![
                        g.summary.tokens.input_tokens as f64,
                        g.summary.tokens.output_tokens as f64,
                        (g.summary.tokens.cache_read_tokens
                            + g.summary.tokens.cache_creation_tokens)
                            as f64,
                        g.summary.tokens.reasoning_tokens as f64,
                    ]
                }
            })
            .collect();
        let highest = values.iter().flatten().copied().fold(0f64, f64::max);
        let highest = if highest > 0. { highest } else { 1. };
        let tick_step = |extent: f64, count: f64| {
            let raw = extent / count;
            let power = 10f64.powf(raw.log10().floor());
            let error = raw / power;
            power
                * if error >= 50f64.sqrt() {
                    10.
                } else if error >= 10f64.sqrt() {
                    5.
                } else if error >= 2f64.sqrt() {
                    2.
                } else {
                    1.
                }
        };
        let nice_step = tick_step(highest, 10.);
        let max = (highest / nice_step).ceil() * nice_step;
        let step = tick_step(max, 4.);
        let ticks: Vec<_> = (0..=(max / step).floor() as usize)
            .map(|i| i as f64 * step)
            .collect();
        let grid_ticks = ticks.clone();
        let count = groups.len();
        let bounds = Rc::new(Cell::new(Bounds::default()));
        let painted = bounds.clone();
        let hover = bounds.clone();
        let cache = self.plot_cache.clone();
        let background: Rgba = crate::settings_style::card_bg().into();
        let border = rgb(BORDER);
        let plot = canvas(
            move |bounds, window, cx| {
                painted.set(bounds);
                let width = (f32::from(bounds.size.width) - 84.).max(1.);
                let height = (f32::from(bounds.size.height) - 32.).max(1.);
                let scale = window.scale_factor();
                let rgb_bytes = |c: Rgba| {
                    [
                        (c.r * 255.).round() as u8,
                        (c.g * 255.).round() as u8,
                        (c.b * 255.).round() as u8,
                    ]
                };
                let key = RasterKey {
                    size: (
                        (width * scale).round().clamp(1., 8192.) as u32,
                        (height * scale).round().clamp(1., 2048.) as u32,
                    ),
                    cells: (
                        (width / 2.).round().clamp(8., 520.) as usize,
                        (height / 2.).round().clamp(8., 200.) as usize,
                    ),
                    scale,
                    cost,
                    max,
                    values: values.clone(),
                    ticks: grid_ticks.clone(),
                    background: rgb_bytes(background),
                    border: rgb_bytes(border),
                };
                let mut cache = cache.borrow_mut();
                if cache.as_ref().is_none_or(|cached| cached.key != key) {
                    let image = raster(&key);
                    if let Some(previous) = cache.replace(Raster { key, image }) {
                        ImageSource::Render(previous.image).evict(None, cx);
                    }
                }
                (cache.as_ref().unwrap().image.clone(), width, height)
            },
            |bounds, (image, width, height), window, _| {
                let _ = window.paint_image(
                    Bounds::new(
                        bounds.origin + point(px(72.), px(10.)),
                        size(px(width), px(height)),
                    ),
                    Corners::default(),
                    image,
                    0,
                    false,
                );
            },
        )
        .size_full();
        let mut chart = div()
            .relative()
            .w_full()
            .h(px(256.))
            .child(plot)
            .id("usage-chart")
            .on_mouse_move(cx.listener(move |this, event: &MouseMoveEvent, _, cx| {
                let b = hover.get();
                let x = f32::from(event.position.x - b.origin.x) - 72.;
                let width = (f32::from(b.size.width) - 84.).max(1.);
                let next = if x >= 0. && x <= width {
                    Some(((x / width) * (count.saturating_sub(1) as f32)).round() as usize)
                } else {
                    None
                };
                if this.hovered != next {
                    this.hovered = next;
                    cx.notify();
                }
            }));
        for value in ticks {
            chart = chart.child(
                label(
                    if cost {
                        money(Some(value))
                    } else {
                        tokens(value.round() as u64)
                    },
                    10.,
                )
                .absolute()
                .left_0()
                .top(px(10. + 224. * (1. - value / max) as f32 - 6.75))
                .w(px(64.))
                .font_family("Geist Mono")
                .text_align(TextAlign::Right)
                .text_color(rgb(MUTED)),
            );
        }
        let mut axis = div()
            .absolute()
            .left(px(72.))
            .right(px(12.))
            .bottom_0()
            .h(px(18.));
        let stride = count.div_ceil(7).max(1);
        for i in (0..count).step_by(stride) {
            let fraction = if count > 1 {
                i as f32 / (count - 1) as f32
            } else {
                0.5
            };
            axis = axis.child(
                column()
                    .absolute()
                    .left(relative(fraction))
                    .w(px(86.))
                    .ml(px(-43.))
                    .items_center()
                    .child(
                        label(groups[i].label.clone(), 10.)
                            .font_family("Geist Mono")
                            .text_color(rgb(MUTED)),
                    ),
            );
        }
        chart.child(axis).into_any_element()
    }
}

#[derive(PartialEq)]
struct RasterKey {
    size: (u32, u32),
    cells: (usize, usize),
    scale: f32,
    cost: bool,
    max: f64,
    values: Vec<Vec<f64>>,
    ticks: Vec<f64>,
    background: [u8; 3],
    border: [u8; 3],
}
pub(super) struct Raster {
    key: RasterKey,
    image: Arc<RenderImage>,
}
fn raster(key: &RasterKey) -> Arc<RenderImage> {
    let (width, height) = key.size;
    let mut pixels = image::RgbaImage::from_pixel(
        width,
        height,
        image::Rgba([key.background[2], key.background[1], key.background[0], 255]),
    );
    let blend = |pixels: &mut image::RgbaImage, x: u32, y: u32, color: [u8; 3], alpha: f32| {
        let p = pixels.get_pixel_mut(x, y);
        for i in 0..3 {
            p[i] = (p[i] as f32 * (1. - alpha) + color[2 - i] as f32 * alpha)
                .round()
                .clamp(0., 255.) as u8;
        }
    };
    for tick in &key.ticks {
        let y = ((1. - *tick / key.max) * height as f64).round().max(0.) as u32;
        for row in y..(y + (key.scale.round() as u32).max(1)).min(height) {
            for x in 0..width {
                if (x as f32 / key.scale) as usize % 6 < 3 {
                    blend(&mut pixels, x, row, key.border, 1.);
                }
            }
        }
    }
    let colors = [
        [53, 143, 243],
        [40, 210, 110],
        [150, 110, 255],
        [255, 150, 50],
    ];
    let bayer = [
        0., 8., 2., 10., 12., 4., 14., 6., 3., 11., 1., 9., 15., 7., 13., 5.,
    ];
    let (cols, rows) = key.cells;
    let count = key.values.len();
    for series in 0..key.values.first().map(|v| v.len()).unwrap_or(0) {
        let dotted = !key.cost && series % 2 == 1;
        for x in 0..cols {
            let position = x as f64 / (cols - 1) as f64 * count.saturating_sub(1) as f64;
            let a = (position.floor() as usize).min(count - 1);
            let b = (a + 1).min(count - 1);
            let t = position - a as f64;
            let value = key.values[a][series] * (1. - t) + key.values[b][series] * t;
            let start = ((1. - value / key.max) * rows as f64)
                .round()
                .clamp(0., rows as f64) as usize;
            let depth = rows - start;
            for y in start..rows {
                let alpha = if y == start {
                    0.72
                } else if y == start + 1 {
                    0.36
                } else {
                    let density = (y - start) as f32 / depth.max(1) as f32;
                    let bias = if dotted { 0.12 } else { 0. } - series as f32 * 0.14;
                    let lit = density > (bayer[(y & 3) * 4 + (x & 3)] + 0.5) / 16. - bias;
                    if dotted && !lit {
                        continue;
                    }
                    (0.3 + density * 0.7) * if lit { 1. } else { 0.4 }
                };
                let left = (x as f32 * width as f32 / cols as f32).round() as u32;
                let right = ((x + 1) as f32 * width as f32 / cols as f32).round() as u32;
                let top = (y as f32 * height as f32 / rows as f32).round() as u32;
                let bottom = ((y + 1) as f32 * height as f32 / rows as f32).round() as u32;
                for py in top..bottom.min(height) {
                    for px in left..right.min(width) {
                        blend(&mut pixels, px, py, colors[series], alpha);
                    }
                }
            }
        }
    }
    Arc::new(RenderImage::new([image::Frame::new(pixels)]))
}
