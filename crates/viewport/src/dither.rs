//! Shared ordered-dither button fill.
use gpui::{IntoElement, Styled, px};
/// One dither cell, in logical pixels.
const DITHER_CELL: f32 = 2.0;
const BAYER_4: [f32; 16] = [
    0.0, 8.0, 2.0, 10.0, 12.0, 4.0, 14.0, 6.0, 3.0, 11.0, 1.0, 9.0, 15.0, 7.0, 13.0, 5.0,
];

/// Whether dither cell `(x, y)` of a `rows`-tall grid is lit, and its alpha.
/// The density ramps from a quarter at the top to full at the bottom
/// (the `gradient` variant), thresholded by a 4x4 Bayer matrix; unlit cells
/// keep a faint 40% wash so the key never reads as holes.
fn dither_cell(x: usize, y: usize, rows: usize) -> (bool, f32) {
    let density = 0.25 + 0.75 * ((y as f32 + 0.5) / rows as f32);
    let threshold = BAYER_4[(y & 3) * 4 + (x & 3)] / 16.0 + 0.5 / 16.0;
    let lit = density > threshold;
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
    let color: gpui::Hsla = color.into();
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
