//! Shared ordered-dither fills: the button texture, the usage meter and charts.
use gpui::{Hsla, IntoElement, Styled, point, px, size};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
/// One dither cell, in logical pixels.
const DITHER_CELL: f32 = 2.0;
const BAYER_4: [f32; 16] = [
    0.0, 8.0, 2.0, 10.0, 12.0, 4.0, 14.0, 6.0, 3.0, 11.0, 1.0, 9.0, 15.0, 7.0, 13.0, 5.0,
];

/// The ordered-dither threshold of cell `(x, y)`, in `0..1`. A cell lights
/// when its density exceeds this.
pub fn threshold(x: usize, y: usize) -> f32 {
    BAYER_4[(y & 3) * 4 + (x & 3)] / 16.0 + 0.5 / 16.0
}

/// Whether dither cell `(x, y)` of a `rows`-tall grid is lit, and its alpha.
/// The density ramps from a quarter at the top to full at the bottom
/// (the `gradient` variant), thresholded by a 4x4 Bayer matrix; unlit cells
/// keep a faint 40% wash so the key never reads as holes.
fn dither_cell(x: usize, y: usize, rows: usize) -> (bool, f32) {
    let density = 0.25 + 0.75 * ((y as f32 + 0.5) / rows as f32);
    let lit = density > threshold(x, y);
    let alpha = (0.3 + density * 0.7) * if lit { 1.0 } else { 0.4 };
    (lit, alpha.clamp(0.0, 1.0))
}

/// Paint an ordered-dither texture over the parent's box: 2px cells, denser
/// toward the bottom, clipped to the parent's rounded corners.
pub fn fill(rgb: [u8; 3], radius: f32) -> impl IntoElement {
    let color = gpui::Rgba {
        r: rgb[0] as f32 / 255.0,
        g: rgb[1] as f32 / 255.0,
        b: rgb[2] as f32 / 255.0,
        a: 1.0,
    };
    fill_color(color.into(), radius)
}

/// [`fill`] in a theme color.
pub fn fill_color(color: Hsla, radius: f32) -> impl IntoElement {
    gpui::canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            let width = f32::from(bounds.size.width);
            let height = f32::from(bounds.size.height);
            let columns = (width / DITHER_CELL).round().max(4.0) as usize;
            let rows = (height / DITHER_CELL).round().max(4.0) as usize;
            let cell_w = width / columns as f32;
            let cell_h = height / rows as f32;
            for y in 0..rows {
                for x in 0..columns {
                    // Skip cells whose centre falls outside the rounded rect.
                    let cx = (x as f32 + 0.5) * cell_w;
                    let cy = (y as f32 + 0.5) * cell_h;
                    let dx = (radius - cx).max(cx - (width - radius)).max(0.0);
                    let dy = (radius - cy).max(cy - (height - radius)).max(0.0);
                    if dx * dx + dy * dy > radius * radius {
                        continue;
                    }
                    // The outer ring of cells draws the key's soft frame.
                    let edge = x == 0 || y == 0 || x + 1 == columns || y + 1 == rows;
                    let alpha = if edge { 0.5 } else { dither_cell(x, y, rows).1 };
                    window.paint_quad(gpui::fill(
                        gpui::Bounds::new(
                            bounds.origin
                                + gpui::point(px(x as f32 * cell_w), px(y as f32 * cell_h)),
                            gpui::size(px(cell_w), px(cell_h)),
                        ),
                        color.opacity(alpha),
                    ));
                }
            }
        },
    )
    .absolute()
    .inset_0()
}

/// How many of `columns` a usage fraction lights. Any usage at all lights one,
/// so a fresh window never reads as unreported.
fn lit_columns(fraction: f32, columns: usize) -> usize {
    (fraction.clamp(0.0, 1.0) * columns as f32).ceil() as usize
}

/// A horizontal usage meter drawn as density: `on` cells thicken toward the
/// reading's leading edge, the rest of the track is a sparse `off` dust.
/// Fills its positioned parent.
pub fn meter(fraction: f32, on: Hsla, off: Hsla) -> impl IntoElement {
    const CELL: f32 = 2.0;
    gpui::canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            let columns = (f32::from(bounds.size.width) / CELL) as usize;
            let rows = (f32::from(bounds.size.height) / CELL) as usize;
            let lit = lit_columns(fraction, columns);
            for y in 0..rows {
                for x in 0..columns {
                    let cutoff = threshold(x, y);
                    let color = if x < lit {
                        let density = 0.5 + 0.5 * (x + 1) as f32 / lit as f32;
                        Some(if density > cutoff {
                            on
                        } else {
                            on.opacity(0.25)
                        })
                    } else {
                        (cutoff < 0.3).then_some(off)
                    };
                    if let Some(color) = color {
                        let origin = point(px(x as f32 * CELL), px(y as f32 * CELL));
                        window.paint_quad(gpui::fill(
                            gpui::Bounds::new(bounds.origin + origin, size(px(CELL), px(CELL))),
                            color,
                        ));
                    }
                }
            }
        },
    )
    .absolute()
    .inset_0()
}

// ---------------------------------------------------------------------------
// Chart dither kit.
//
// A CPU raster port of the canvas chart fills: every cell of a low-resolution
// backing grid (`backing_size`) is composited source-over in straight-alpha
// RGBA, exactly like `fillRect` on a 2D canvas, then the whole grid is shown
// as one image stretched over the plot with a nearest-neighbour look.
// ---------------------------------------------------------------------------

/// 4x4 ordered-dither thresholds, `(v + 0.5) / 16`.
pub const BAYER: [[f64; 4]; 4] = {
    const M: [[f64; 4]; 4] = [
        [0.0, 8.0, 2.0, 10.0],
        [12.0, 4.0, 14.0, 6.0],
        [3.0, 11.0, 1.0, 9.0],
        [15.0, 7.0, 13.0, 5.0],
    ];
    let mut out = [[0.0; 4]; 4];
    let mut y = 0;
    while y < 4 {
        let mut x = 0;
        while x < 4 {
            out[y][x] = (M[y][x] + 0.5) / 16.0;
            x += 1;
        }
        y += 1;
    }
    out
};
/// One backing cell, in logical pixels.
pub const CELL: f64 = 2.0;
pub const MAX_COLS: usize = 520;
pub const MAX_ROWS: usize = 200;
/// Alpha of the series-coloured cap drawn on top of each column.
pub const BORDER_ALPHA: f64 = 0.72;
/// Relative alpha of unlit cells, so the fill never reads as holes.
pub const OFF_TIER: f64 = 0.4;

pub type Rgb = [u8; 3];

/// Fill, line and star colours for one series hue.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Seed {
    pub fill: Rgb,
    pub line: Rgb,
    pub star: Rgb,
}

/// Named series hues.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DitherColor {
    Green,
    Blue,
    Purple,
    Pink,
    Orange,
    Red,
    Grey,
}

impl DitherColor {
    pub const ALL: [DitherColor; 7] = [
        DitherColor::Green,
        DitherColor::Blue,
        DitherColor::Purple,
        DitherColor::Pink,
        DitherColor::Orange,
        DitherColor::Red,
        DitherColor::Grey,
    ];

    pub fn seed(self) -> Seed {
        PALETTE[self as usize].1
    }

    pub fn from_name(name: &str) -> Option<Self> {
        PALETTE
            .iter()
            .find(|(color, _)| color.name() == name)
            .map(|(color, _)| *color)
    }

    pub fn name(self) -> &'static str {
        match self {
            DitherColor::Green => "green",
            DitherColor::Blue => "blue",
            DitherColor::Purple => "purple",
            DitherColor::Pink => "pink",
            DitherColor::Orange => "orange",
            DitherColor::Red => "red",
            DitherColor::Grey => "grey",
        }
    }
}

/// Palette in `DitherColor` declaration order.
pub const PALETTE: [(DitherColor, Seed); 7] = [
    (
        DitherColor::Green,
        Seed {
            fill: [40, 210, 110],
            line: [150, 255, 180],
            star: [200, 255, 220],
        },
    ),
    (
        DitherColor::Blue,
        Seed {
            fill: [53, 143, 243],
            line: [150, 200, 255],
            star: [205, 228, 255],
        },
    ),
    (
        DitherColor::Purple,
        Seed {
            fill: [150, 110, 255],
            line: [200, 175, 255],
            star: [225, 210, 255],
        },
    ),
    (
        DitherColor::Pink,
        Seed {
            fill: [240, 90, 190],
            line: [255, 170, 220],
            star: [255, 205, 235],
        },
    ),
    (
        DitherColor::Orange,
        Seed {
            fill: [255, 150, 50],
            line: [255, 195, 130],
            star: [255, 220, 175],
        },
    ),
    (
        DitherColor::Red,
        Seed {
            fill: [240, 70, 70],
            line: [255, 150, 140],
            star: [255, 195, 185],
        },
    ),
    (
        DitherColor::Grey,
        Seed {
            fill: [92, 92, 100],
            line: [140, 140, 150],
            star: [165, 165, 175],
        },
    ),
];

/// Donut slice colours, in slice order.
pub const DONUT_COLORS: [DitherColor; 6] = [
    DitherColor::Blue,
    DitherColor::Orange,
    DitherColor::Green,
    DitherColor::Purple,
    DitherColor::Pink,
    DitherColor::Red,
];

/// Fill style of a dithered column.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Variant {
    #[default]
    Gradient,
    Solid,
    Dotted,
    Hatched,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PaintOpts {
    pub variant: Variant,
    /// Hover intensity, 0..=1.
    pub intensity: f64,
    /// Emphasis multiplier: 1 normally, 0.3 for de-emphasised series.
    pub dim: f64,
    pub stacked: bool,
    /// Thins the scatter of later series so overlaps stay legible.
    pub sparse: f64,
}

impl Default for PaintOpts {
    fn default() -> Self {
        Self {
            variant: Variant::Gradient,
            intensity: 0.0,
            dim: 1.0,
            stacked: false,
            sparse: 0.0,
        }
    }
}

/// `Math.round`: halves round toward positive infinity.
fn js_round(value: f64) -> f64 {
    (value + 0.5).floor()
}

fn clamp01(value: f64) -> f64 {
    value.clamp(0.0, 1.0)
}

/// Shared density-to-alpha scatter for Cartesian and radial fills. `depth` is
/// 0 at the value edge and 1 at the floor.
pub fn cell_alpha(x: i32, y: i32, depth: f64, opts: &PaintOpts) -> f32 {
    cell_alpha_f64(x, y, depth, opts) as f32
}

fn cell_alpha_f64(x: i32, y: i32, depth: f64, opts: &PaintOpts) -> f64 {
    let PaintOpts {
        variant,
        intensity,
        dim,
        stacked,
        sparse,
    } = *opts;
    let mut density = depth;
    if stacked {
        density = 0.5 + 0.5 * density;
    }
    if variant == Variant::Hatched && ((x + y) & 3) >= 2 {
        return 0.0;
    }
    let bias = if variant == Variant::Dotted {
        0.12
    } else {
        0.0
    } + if stacked { 0.2 } else { 0.0 }
        - sparse;
    let lit = variant == Variant::Solid
        || density > BAYER[(y & 3) as usize][(x & 3) as usize] - 0.1 * intensity - bias;
    if variant == Variant::Dotted && !lit {
        return 0.0;
    }
    let k = (0.3 + density * 0.7) * (1.0 + 0.22 * intensity);
    clamp01(if lit { k } else { k * OFF_TIER } * dim)
}

/// Linear-resample a per-index array to `cols` columns.
pub fn resample(src: &[f64], cols: usize) -> Vec<f64> {
    let last = src.len().saturating_sub(1).max(1) as f64;
    (0..cols)
        .map(|c| {
            let t = (c as f64 / cols.saturating_sub(1).max(1) as f64) * last;
            let i = t.floor() as usize;
            let f = t - i as f64;
            let a = src.get(i).copied().unwrap_or(0.0);
            let b = src
                .get((i + 1).min(src.len().saturating_sub(1)))
                .copied()
                .unwrap_or(a);
            a + (b - a) * f
        })
        .collect()
}

/// Backing-grid resolution for a plot rect in logical pixels.
pub fn backing_size(width: f64, height: f64) -> (usize, usize) {
    (
        js_round(width / CELL).max(8.0).min(MAX_COLS as f64) as usize,
        js_round(height / CELL).max(8.0).min(MAX_ROWS as f64) as usize,
    )
}

/// A low-resolution straight-alpha RGBA grid, one pixel per dither cell.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct DitherRaster {
    pub cols: usize,
    pub rows: usize,
    pub pixels: Vec<[u8; 4]>,
}

impl DitherRaster {
    /// A fully transparent grid.
    pub fn new(cols: usize, rows: usize) -> Self {
        Self {
            cols,
            rows,
            pixels: vec![[0; 4]; cols * rows],
        }
    }

    pub fn pixel(&self, x: usize, y: usize) -> [u8; 4] {
        self.pixels[y * self.cols + x]
    }

    pub fn is_empty(&self) -> bool {
        self.pixels.iter().all(|p| p[3] == 0)
    }

    /// `fillRect(x, y, 1, 1)` with `rgba(rgb, alpha)`: source-over in straight
    /// alpha, clipped to the grid like a canvas.
    pub fn blend(&mut self, x: i32, y: i32, rgb: Rgb, alpha: f64) {
        if x < 0 || y < 0 || x as usize >= self.cols || y as usize >= self.rows {
            return;
        }
        // The canvas quantises the fill colour's alpha to a byte.
        let sa = (clamp01(alpha) * 255.0).round() / 255.0;
        if sa <= 0.0 {
            return;
        }
        let dst = &mut self.pixels[y as usize * self.cols + x as usize];
        let da = dst[3] as f64 / 255.0;
        let out_a = sa + da * (1.0 - sa);
        let mut out = [0u8; 4];
        for i in 0..3 {
            let c = (rgb[i] as f64 * sa + dst[i] as f64 * da * (1.0 - sa)) / out_a;
            out[i] = c.round().clamp(0.0, 255.0) as u8;
        }
        out[3] = (out_a * 255.0).round().clamp(0.0, 255.0) as u8;
        *dst = out;
    }

    /// The grid as BGRA bytes, one pixel per cell (what `gpui::RenderImage`
    /// expects; straight alpha).
    pub fn to_bgra(&self) -> Vec<u8> {
        self.pixels
            .iter()
            .flat_map(|[r, g, b, a]| [*b, *g, *r, *a])
            .collect()
    }

    /// The grid nearest-neighbour upscaled to exactly `width x height` device
    /// pixels, as BGRA bytes. GPUI samples images linearly, so painting the
    /// image at its native device size is what keeps cell edges crisp.
    pub fn upscaled_bgra(&self, width: u32, height: u32) -> Vec<u8> {
        let (width, height) = (width.max(1) as usize, height.max(1) as usize);
        let mut out = Vec::with_capacity(width * height * 4);
        if self.cols == 0 || self.rows == 0 {
            out.resize(width * height * 4, 0);
            return out;
        }
        let xs: Vec<usize> = (0..width)
            .map(|px| (px * self.cols / width).min(self.cols - 1))
            .collect();
        for py in 0..height {
            let row = (py * self.rows / height).min(self.rows - 1) * self.cols;
            for &x in &xs {
                let [r, g, b, a] = self.pixels[row + x];
                out.extend_from_slice(&[b, g, r, a]);
            }
        }
        out
    }

    /// A `gpui::RenderImage` of the grid at `width x height` device pixels.
    pub fn render_image(&self, width: u32, height: u32) -> Arc<gpui::RenderImage> {
        let (w, h) = (width.max(1), height.max(1));
        let buffer = image::RgbaImage::from_raw(w, h, self.upscaled_bgra(w, h))
            .expect("buffer length matches dimensions");
        Arc::new(gpui::RenderImage::new([image::Frame::new(buffer)]))
    }

    fn fingerprint(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.hash(&mut hasher);
        hasher.finish()
    }
}

/// Fill column `x` from row `top` down to `floor` with the ordered-dither
/// scatter (solid at the floor, dissolving toward the value line), then cap it
/// with a soft border in the series colour.
pub fn paint_column(
    raster: &mut DitherRaster,
    x: i32,
    top: f64,
    floor: f64,
    rgb: Rgb,
    opts: &PaintOpts,
) {
    let dim = opts.dim;
    let t = js_round(top) as i32;
    let f = js_round(floor) as i32;
    let depth = f - t;
    if depth <= 0 {
        raster.blend(x, t, rgb, BORDER_ALPHA * dim);
        return;
    }
    for y in t..f {
        let alpha = cell_alpha_f64(x, y, (y - t) as f64 / depth as f64, opts);
        if alpha == 0.0 {
            continue;
        }
        raster.blend(x, y, rgb, alpha);
    }
    raster.blend(x, t, rgb, BORDER_ALPHA * dim);
    if depth > 1 {
        raster.blend(x, t + 1, rgb, BORDER_ALPHA * 0.5 * dim);
    }
}

/// A horizontal bar covering `[start_frac, start_frac + fraction]` of the
/// width, full height, in the gradient scatter.
pub fn bar_fill(
    cols: usize,
    rows: usize,
    start_frac: f64,
    fraction: f64,
    rgb: Rgb,
) -> DitherRaster {
    let mut raster = DitherRaster::new(cols, rows);
    let left = js_round(start_frac * cols as f64) as i32;
    let right = js_round((start_frac + fraction) * cols as f64) as i32;
    for x in left..right {
        paint_column(&mut raster, x, 0.0, rows as f64, rgb, &PaintOpts::default());
    }
    raster
}

/// A donut ring with one slice per value, from 12 o'clock clockwise in
/// `DONUT_COLORS` order. A zero total paints nothing.
pub fn donut_fill(cols: usize, rows: usize, values: &[f64]) -> DitherRaster {
    use std::f64::consts::PI;
    let mut raster = DitherRaster::new(cols, rows);
    let total: f64 = values.iter().sum();
    if total == 0.0 {
        return raster;
    }
    let mut cumulative = 0.0;
    let ends: Vec<f64> = values
        .iter()
        .map(|value| {
            cumulative += value / total;
            cumulative
        })
        .collect();
    let radius = cols.min(rows) as f64;
    let outer = radius * 56.0 / 120.0;
    let inner = radius * 44.0 / 120.0;
    let opts = PaintOpts::default();
    for y in 0..rows {
        for x in 0..cols {
            let dx = x as f64 + 0.5 - cols as f64 / 2.0;
            let dy = y as f64 + 0.5 - rows as f64 / 2.0;
            let distance = dx.hypot(dy);
            if distance < inner || distance > outer {
                continue;
            }
            let angle = ((dy.atan2(dx) + PI / 2.0 + PI * 2.0) % (PI * 2.0)) / (PI * 2.0);
            let index = ends.iter().position(|end| angle < *end).unwrap_or(0);
            let seed = DONUT_COLORS[index % DONUT_COLORS.len()].seed();
            let depth = outer - distance;
            let alpha = if depth < 1.0 {
                BORDER_ALPHA
            } else {
                cell_alpha_f64(x as i32, y as i32, depth / (outer - inner), &opts)
            };
            raster.blend(x as i32, y as i32, seed.fill, alpha);
        }
    }
    raster
}

/// One series' `[top, floor]` band per backing column, in rows.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Band {
    pub top: Vec<f64>,
    pub floor: Vec<f64>,
}

/// Glow depth under a line, in rows.
pub fn line_glow(rows: usize) -> f64 {
    js_round(rows as f64 * 0.16).max(6.0)
}

/// The glow band hugging a line whose per-point `tops` are already in rows
/// (0 = top edge, `rows - 1` = bottom), resampled to `cols`.
pub fn line_band(tops: &[f64], cols: usize, rows: usize) -> Band {
    let glow = line_glow(rows);
    let bottom = rows.saturating_sub(1) as f64;
    let floor: Vec<f64> = tops.iter().map(|top| bottom.min(top + glow)).collect();
    Band {
        top: resample(tops, cols),
        floor: resample(&floor, cols),
    }
}

/// Per-frame settings for line fills.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LineOpts {
    pub variant: Variant,
    /// Hover intensity, 0..=1.
    pub intensity: f64,
    /// The selected/focused series; every other series is dimmed to 0.3.
    pub emphasis: Option<usize>,
    /// Entrance reveal progress, 0..=1 (already eased).
    pub reveal: f64,
}

impl Default for LineOpts {
    fn default() -> Self {
        Self {
            variant: Variant::Gradient,
            intensity: 0.0,
            emphasis: None,
            reveal: 1.0,
        }
    }
}

/// Paint precomputed bands; later series get a sparser scatter.
pub fn paint_bands(raster: &mut DitherRaster, bands: &[(&Band, Rgb)], opts: &LineOpts) {
    let cols = raster.cols;
    let reveal_cols = (opts.reveal * cols as f64).ceil();
    for (si, (band, rgb)) in bands.iter().enumerate() {
        let dim = match opts.emphasis {
            Some(key) if key != si => 0.3,
            _ => 1.0,
        };
        let paint = PaintOpts {
            variant: opts.variant,
            intensity: opts.intensity,
            dim,
            stacked: false,
            sparse: si as f64 * 0.14,
        };
        for x in 0..cols {
            if x as f64 > reveal_cols {
                break;
            }
            let a = band.top.get(x).copied().unwrap_or(0.0);
            let b = band.floor.get(x).copied().unwrap_or(0.0);
            paint_column(raster, x as i32, a.min(b), a.max(b), *rgb, &paint);
        }
    }
}

/// Line chart glow bands: `series` holds each line's per-point tops (in rows)
/// and its fill colour.
pub fn line_fill(
    cols: usize,
    rows: usize,
    series: &[(Vec<f64>, Rgb)],
    opts: &LineOpts,
) -> DitherRaster {
    let bands: Vec<(Band, Rgb)> = series
        .iter()
        .map(|(tops, rgb)| (line_band(tops, cols, rows), *rgb))
        .collect();
    let refs: Vec<(&Band, Rgb)> = bands.iter().map(|(band, rgb)| (band, *rgb)).collect();
    let mut raster = DitherRaster::new(cols, rows);
    paint_bands(&mut raster, &refs, opts);
    raster
}

/// Backing column of data point `index` out of `data_len`.
pub fn data_column(index: usize, data_len: usize, cols: usize) -> i32 {
    if data_len <= 1 {
        return -1;
    }
    js_round(index as f64 / (data_len - 1) as f64 * cols.saturating_sub(1) as f64) as i32
}

/// Scrub crosshair: a 0.55-alpha column from the line down, plus a solid 3x3
/// dot on the line.
pub fn hover_column(raster: &mut DitherRaster, col: i32, top: f64, rgb: Rgb) {
    let my = js_round(top) as i32;
    for y in my..raster.rows as i32 {
        raster.blend(col, y, rgb, 0.55);
    }
    for dy in -1..=1 {
        for dx in -1..=1 {
            raster.blend(col + dx, my + dy, rgb, 1.0);
        }
    }
}

/// Twinkling stars scattered inside each band. `tick` advances every 100ms;
/// with `reduced_motion` the stars hold steady. Star positions are
/// deterministic per series index.
#[allow(clippy::too_many_arguments)]
pub fn stars(
    raster: &mut DitherRaster,
    bands: &[(&Band, Rgb)],
    data_len: usize,
    tick: u64,
    intensity: f64,
    reduced_motion: bool,
    reveal: f64,
) {
    let cols = raster.cols;
    let rows = raster.rows as i32;
    let reveal_cols = reveal * cols as f64;
    let per = js_round(cols as f64 / 14.0).max(4.0) as usize;
    for (k, (band, rgb)) in bands.iter().enumerate() {
        for i in 0..per {
            let seed = i * 67 + 13 + k * 131;
            let xi = seed % data_len.max(1);
            let depth = ((seed * 53 + 7) % 100) as f64 / 100.0;
            let phase = ((seed * 41) % 360) as f64;
            let sx = js_round(
                xi as f64 / data_len.saturating_sub(1).max(1) as f64
                    * cols.saturating_sub(1) as f64,
            );
            if sx > reveal_cols {
                continue;
            }
            let sx_index = sx as usize;
            let top = band.top.get(sx_index).copied().unwrap_or(0.0);
            let floor = band
                .floor
                .get(sx_index)
                .copied()
                .unwrap_or((rows - 1) as f64);
            let sy = js_round(top + depth * (floor - top)) as i32;
            let tw = if reduced_motion {
                0.85
            } else {
                (((tick as f64 + phase) * 0.35).sin() + 1.0) / 2.0
            };
            let lift = tw * (0.7 + 0.3 * intensity);
            if lift < 0.55 || sy < 0 || sy >= rows {
                continue;
            }
            let sx = sx as i32;
            raster.blend(sx, sy, *rgb, lift);
            if tw > 0.9 {
                let glint = lift * 0.6 * (tw - 0.9) * 10.0;
                raster.blend(sx - 1, sy, *rgb, glint);
                raster.blend(sx + 1, sy, *rgb, glint);
                raster.blend(sx, sy - 1, *rgb, glint);
                raster.blend(sx, sy + 1, *rgb, glint);
            }
        }
    }
}

/// Holds the GPU image for the last painted raster so repeated frames reuse
/// one atlas tile; a changed raster or size evicts the old tile.
#[derive(Default)]
pub struct RasterImageCache {
    cached: Option<(u64, (u32, u32), Arc<gpui::RenderImage>)>,
}

impl RasterImageCache {
    /// The image for `raster` at `size` device pixels, rebuilding it only when
    /// either changed.
    pub fn image(
        &mut self,
        raster: &DitherRaster,
        size: (u32, u32),
        window: &mut gpui::Window,
        cx: &mut gpui::App,
    ) -> Arc<gpui::RenderImage> {
        let fingerprint = raster.fingerprint();
        match &self.cached {
            Some((key, cached_size, image)) if *key == fingerprint && *cached_size == size => {
                image.clone()
            }
            _ => {
                let image = raster.render_image(size.0, size.1);
                if let Some((_, _, old)) = self.cached.replace((fingerprint, size, image.clone())) {
                    cx.drop_image(old, Some(window));
                }
                image
            }
        }
    }

    /// Free the cached atlas tile (call when the owner goes away).
    pub fn release(&mut self, cx: &mut gpui::App) {
        if let Some((_, _, image)) = self.cached.take() {
            cx.drop_image(image, None);
        }
    }
}

/// Paint `raster` as one image stretched over `bounds`, rasterised at the
/// window's device resolution with nearest-neighbour cells.
pub fn paint_raster(
    bounds: gpui::Bounds<gpui::Pixels>,
    raster: &DitherRaster,
    cache: &mut RasterImageCache,
    window: &mut gpui::Window,
    cx: &mut gpui::App,
) {
    let scale = window.scale_factor();
    let width = (f32::from(bounds.size.width) * scale)
        .round()
        .clamp(1.0, 8192.0) as u32;
    let height = (f32::from(bounds.size.height) * scale)
        .round()
        .clamp(1.0, 8192.0) as u32;
    let image = cache.image(raster, (width, height), window, cx);
    let _ = window.paint_image(bounds, gpui::Corners::default(), image, 0, false);
}

/// An absolutely positioned element filling its parent with `raster`.
pub fn raster_canvas(
    raster: Arc<DitherRaster>,
    cache: Rc<RefCell<RasterImageCache>>,
) -> impl IntoElement {
    gpui::canvas(
        |_, _, _| {},
        move |bounds, _, window, cx| {
            paint_raster(bounds, &raster, &mut cache.borrow_mut(), window, cx);
        },
    )
    .absolute()
    .inset_0()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meter_lights_any_usage() {
        assert_eq!(lit_columns(0.0, 100), 0);
        assert_eq!(lit_columns(0.001, 100), 1);
        assert_eq!(lit_columns(0.5, 100), 50);
        assert_eq!(lit_columns(2.0, 100), 100);
    }

    fn samples(depth: f64, opts: PaintOpts) -> Vec<f32> {
        (0..16)
            .map(|i| cell_alpha(i % 4, i / 4, depth, &opts))
            .collect()
    }

    fn mean(values: &[f32]) -> f32 {
        values.iter().sum::<f32>() / values.len() as f32
    }

    fn alpha(raster: &DitherRaster, x: usize, y: usize) -> f64 {
        raster.pixel(x, y)[3] as f64 / 255.0
    }

    #[test]
    fn button_fill_scatter_is_unchanged() {
        // Values of the send key's original ramp, recomputed independently.
        for rows in [4usize, 9, 20] {
            for y in 0..rows {
                for x in 0..8 {
                    let density = 0.25 + 0.75 * ((y as f32 + 0.5) / rows as f32);
                    let threshold = BAYER_4[(y & 3) * 4 + (x & 3)] / 16.0 + 0.5 / 16.0;
                    let lit = density > threshold;
                    let expected =
                        ((0.3 + density * 0.7) * if lit { 1.0 } else { 0.4 }).clamp(0.0, 1.0);
                    assert_eq!(dither_cell(x, y, rows), (lit, expected));
                }
            }
        }
        assert_eq!(
            dither_cell(0, 0, 4),
            (true, 0.3 + (0.25 + 0.75 / 8.0) * 0.7)
        );
        assert_eq!(DITHER_CELL, 2.0);
    }

    #[test]
    fn bayer_thresholds() {
        assert_eq!(BAYER[0][0], 0.5 / 16.0);
        assert_eq!(BAYER[3][0], 15.5 / 16.0);
        assert_eq!(BAYER[2][2], 1.5 / 16.0);
    }

    #[test]
    fn scatter_densifies_toward_the_floor() {
        let opts = PaintOpts::default();
        assert!(mean(&samples(0.0, opts)) < mean(&samples(0.5, opts)));
        assert!(mean(&samples(0.5, opts)) < mean(&samples(1.0, opts)));
        assert_eq!(samples(1.0, opts), vec![1.0; 16]);
    }

    #[test]
    fn variants_dimming_and_stacking() {
        let opts = PaintOpts::default();
        let dotted = PaintOpts {
            variant: Variant::Dotted,
            ..opts
        };
        assert!(samples(0.1, dotted).contains(&0.0));
        let solid = samples(
            0.5,
            PaintOpts {
                variant: Variant::Solid,
                ..opts
            },
        );
        assert!(solid.iter().all(|a| *a == solid[0]));
        for depth in [0.0, 0.5, 1.0] {
            for a in samples(
                depth,
                PaintOpts {
                    intensity: 1.0,
                    ..opts
                },
            ) {
                assert!((0.0..=1.0).contains(&a));
            }
        }
        let dimmed = mean(&samples(0.5, PaintOpts { dim: 0.3, ..opts }));
        assert!((dimmed - mean(&samples(0.5, opts)) * 0.3).abs() < 1e-5);
        assert!(
            mean(&samples(
                0.1,
                PaintOpts {
                    stacked: true,
                    ..opts
                }
            )) > mean(&samples(0.1, opts))
        );
        let hatched = PaintOpts {
            variant: Variant::Hatched,
            ..opts
        };
        assert_eq!(cell_alpha(1, 1, 1.0, &hatched), 0.0);
        assert_eq!(cell_alpha(0, 0, 1.0, &hatched), 1.0);
    }

    #[test]
    fn zero_depth_column_is_the_cap() {
        let mut raster = DitherRaster::new(4, 8);
        paint_column(
            &mut raster,
            1,
            3.0,
            3.0,
            [10, 20, 30],
            &PaintOpts::default(),
        );
        assert_eq!(
            raster.pixel(1, 3),
            [10, 20, 30, (0.72f64 * 255.0).round() as u8]
        );
        assert_eq!(raster.pixels.iter().filter(|p| p[3] > 0).count(), 1);
    }

    #[test]
    fn depth_one_column_compounds_scatter_under_the_cap() {
        let mut raster = DitherRaster::new(4, 8);
        let opts = PaintOpts::default();
        paint_column(&mut raster, 0, 5.0, 6.0, [10, 20, 30], &opts);
        let scatter = (cell_alpha_f64(0, 5, 0.0, &opts) * 255.0).round() / 255.0;
        let cap = (BORDER_ALPHA * 255.0).round() / 255.0;
        let expected = scatter + cap * (1.0 - scatter);
        assert!((alpha(&raster, 0, 5) - expected).abs() <= 1.0 / 255.0);
        assert!(alpha(&raster, 0, 5) >= cap);
        assert_eq!(raster.pixel(0, 5)[..3], [10, 20, 30]);
        assert_eq!(raster.pixel(0, 6)[3], 0);
    }

    #[test]
    fn column_mean_alpha_rises_with_depth() {
        let mut raster = DitherRaster::new(8, 64);
        for x in 0..8 {
            paint_column(
                &mut raster,
                x,
                0.0,
                64.0,
                [255, 255, 255],
                &PaintOpts::default(),
            );
        }
        let row_mean = |y: usize| (0..8).map(|x| alpha(&raster, x, y)).sum::<f64>() / 8.0;
        let bands: Vec<f64> = (0..4)
            .map(|b| (b * 16 + 2..b * 16 + 16).map(row_mean).sum::<f64>())
            .collect();
        assert!(bands.windows(2).all(|w| w[0] < w[1]), "{bands:?}");
    }

    #[test]
    fn bar_covers_its_span() {
        let raster = bar_fill(20, 10, 0.25, 0.5, DitherColor::Green.seed().fill);
        for x in 0..20 {
            let painted = (0..10).any(|y| raster.pixel(x, y)[3] > 0);
            assert_eq!(painted, (5..15).contains(&x), "column {x}");
        }
    }

    #[test]
    fn donut_paints_only_the_ring() {
        let (cols, rows) = (60, 40);
        let raster = donut_fill(cols, rows, &[1.0, 2.0, 3.0]);
        let radius = cols.min(rows) as f64;
        let (outer, inner) = (radius * 56.0 / 120.0, radius * 44.0 / 120.0);
        let mut painted = 0;
        for y in 0..rows {
            for x in 0..cols {
                let d =
                    (x as f64 + 0.5 - cols as f64 / 2.0).hypot(y as f64 + 0.5 - rows as f64 / 2.0);
                if raster.pixel(x, y)[3] > 0 {
                    painted += 1;
                    assert!(d >= inner && d <= outer, "({x},{y}) at {d}");
                }
            }
        }
        assert!(painted > 0);
        // The first slice starts at 12 o'clock and runs clockwise.
        let top = raster.pixel(cols / 2, (rows as f64 / 2.0 - outer + 0.5) as usize);
        assert_eq!(top[..3], DONUT_COLORS[0].seed().fill);
    }

    #[test]
    fn donut_with_zero_total_is_empty() {
        assert!(donut_fill(40, 40, &[0.0, 0.0]).is_empty());
        assert!(donut_fill(40, 40, &[]).is_empty());
    }

    #[test]
    fn backing_size_clamps() {
        assert_eq!(backing_size(0.0, 0.0), (8, 8));
        assert_eq!(backing_size(100.0, 60.0), (50, 30));
        assert_eq!(backing_size(5000.0, 5000.0), (MAX_COLS, MAX_ROWS));
        assert_eq!(backing_size(41.0, 43.0), (21, 22));
    }

    #[test]
    fn resample_interpolates_linearly() {
        assert_eq!(resample(&[0.0, 10.0], 3), vec![0.0, 5.0, 10.0]);
        // A single point keeps the reference quirk: the last column reads
        // past the end and falls back to 0.
        assert_eq!(resample(&[4.0], 3), vec![4.0, 4.0, 0.0]);
        assert_eq!(resample(&[], 2), vec![0.0, 0.0]);
    }

    #[test]
    fn line_fill_glows_under_the_line() {
        let (cols, rows) = (40, 50);
        let glow = line_glow(rows);
        assert_eq!(glow, 8.0);
        let raster = line_fill(
            cols,
            rows,
            &[(vec![10.0, 10.0], [0, 0, 255])],
            &LineOpts::default(),
        );
        for x in 0..cols {
            assert!(raster.pixel(x, 10)[3] > 0);
            assert_eq!(raster.pixel(x, 9)[3], 0);
            assert_eq!(raster.pixel(x, 18)[3], 0);
        }
    }

    #[test]
    fn hover_and_stars_stay_in_bounds() {
        let (cols, rows) = (30, 20);
        let band = line_band(&[5.0, 8.0, 3.0], cols, rows);
        let mut raster = DitherRaster::new(cols, rows);
        hover_column(&mut raster, 0, 5.0, [255, 0, 0]);
        assert_eq!(raster.pixel(0, 5), [255, 0, 0, 255]);
        assert_eq!(raster.pixel(0, 19)[3], (0.55f64 * 255.0).round() as u8);
        assert_eq!(raster.pixel(0, 3)[3], 0);
        let before = raster.clone();
        stars(
            &mut raster,
            &[(&band, [255, 255, 255])],
            3,
            0,
            1.0,
            true,
            1.0,
        );
        assert_ne!(raster, before);
        let mut again = before.clone();
        stars(
            &mut again,
            &[(&band, [255, 255, 255])],
            3,
            0,
            1.0,
            true,
            1.0,
        );
        assert_eq!(raster, again);
    }

    #[test]
    fn upscale_is_nearest_neighbour_bgra() {
        let mut raster = DitherRaster::new(2, 1);
        raster.pixels = vec![[1, 2, 3, 4], [5, 6, 7, 8]];
        assert_eq!(raster.to_bgra(), vec![3, 2, 1, 4, 7, 6, 5, 8]);
        let up = raster.upscaled_bgra(4, 2);
        assert_eq!(up.len(), 4 * 2 * 4);
        assert_eq!(
            &up[0..16],
            &[3, 2, 1, 4, 3, 2, 1, 4, 7, 6, 5, 8, 7, 6, 5, 8]
        );
        assert_eq!(&up[0..16], &up[16..32]);
    }

    #[test]
    fn palette_order_matches_enum() {
        for (i, (color, _)) in PALETTE.iter().enumerate() {
            assert_eq!(DitherColor::ALL[i], *color);
            assert_eq!(DitherColor::from_name(color.name()), Some(*color));
        }
        assert_eq!(DitherColor::Blue.seed().fill, [53, 143, 243]);
    }
}
