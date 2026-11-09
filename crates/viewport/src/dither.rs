//! Shared ordered-dither fills: the button texture and the usage meter.
use gpui::{Hsla, IntoElement, Styled, point, px, size};
/// The olive of the send key and every other main action.
pub const KEY_RGB: [u8; 3] = [151, 183, 76];
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
}
