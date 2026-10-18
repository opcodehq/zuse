//! Shared application action treatment for native and browser surfaces.
use gpui::{prelude::*, *};

/// Two-pixel ordered dither, with a bottom-weighted fill and pointer emphasis.
/// Painted as GPU quads; there is no bitmap allocation or idle animation loop.
pub fn dither_background(primary: bool, enabled: bool) -> AnyElement {
    canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            let width = f32::from(bounds.size.width);
            let height = f32::from(bounds.size.height);
            let columns = (width / 2.).ceil() as usize;
            let rows = (height / 2.).ceil() as usize;
            let intensity = if enabled && bounds.contains(&window.mouse_position()) {
                1.0
            } else {
                0.0
            };
            let seed = if primary { 0x97b74c } else { 0x5c6068 };
            let bayer = [
                0., 8., 2., 10., 12., 4., 14., 6., 3., 11., 1., 9., 15., 7., 13., 5.,
            ];
            for y in 0..rows {
                let density = 0.25 + 0.75 * (y as f32 + 0.5) / rows.max(1) as f32;
                for x in 0..columns {
                    let threshold = (bayer[(y & 3) * 4 + (x & 3)] + 0.5) / 16.;
                    let lit = density > threshold - intensity * 0.1;
                    let border = x == 0 || y == 0 || x + 1 == columns || y + 1 == rows;
                    let tier = if lit { 1. } else { 0.4 };
                    let alpha = if border {
                        0.5 + 0.25 * intensity
                    } else {
                        ((0.3 + density * 0.7) * tier * (1. + intensity * 0.22)).clamp(0., 1.)
                    };
                    let origin = bounds.origin + point(px(x as f32 * 2.), px(y as f32 * 2.));
                    let cell = Bounds::new(
                        origin,
                        size(
                            px(2f32.min(width - x as f32 * 2.)),
                            px(2f32.min(height - y as f32 * 2.)),
                        ),
                    );
                    window.paint_quad(fill(cell, rgba((seed << 8) | ((alpha * 255.) as u32))));
                }
            }
        },
    )
    .absolute()
    .inset_0()
    .size_full()
    .into_any_element()
}

pub fn dither_action(
    id: impl Into<ElementId>,
    name: impl Into<SharedString>,
    enabled: bool,
    primary: bool,
) -> Stateful<Div> {
    let name = name.into();
    div()
        .id(id)
        .role(Role::Button)
        .aria_label(name.clone())
        .relative()
        .h(px(28.))
        .px_3()
        .rounded(px(6.))
        .overflow_hidden()
        .flex()
        .items_center()
        .justify_center()
        .gap_2()
        .text_size(px(11.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(rgb(0xffffff))
        .bg(rgb(0x13150f))
        .when(!enabled, |d| d.opacity(0.45))
        .when(enabled, |d| d.cursor_pointer().hover(|d| d.opacity(0.95)))
        .child(dither_background(primary, enabled))
        .child(div().relative().child(name))
}

pub fn dither_icon_action(
    id: impl Into<ElementId>,
    glyph: &str,
    label: &'static str,
    enabled: bool,
    primary: bool,
) -> Stateful<Div> {
    div()
        .id(id)
        .role(Role::Button)
        .aria_label(label)
        .relative()
        .size(px(28.))
        .rounded(px(10.))
        .overflow_hidden()
        .flex()
        .items_center()
        .justify_center()
        .bg(rgb(0x13150f))
        .when(!enabled, |d| d.opacity(0.45))
        .when(enabled, |d| d.cursor_pointer().hover(|d| d.opacity(0.95)))
        .child(
            div()
                .absolute()
                .inset_0()
                .opacity(0.32)
                .child(dither_background(primary, enabled)),
        )
        .child(
            svg()
                .path(SharedString::from(format!("icons/{glyph}.svg")))
                .size(px(14.))
                .text_color(rgb(0xffffff)),
        )
}
