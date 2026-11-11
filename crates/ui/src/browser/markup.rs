//! Screenshot markup: select a region and draw over a frozen capture.
//!
//! Platforms only capture the page as pixels. Editing, painting and export
//! are shared, so every platform gets the same tools. Marks live in image
//! pixels; the screen and the saved PNG both stroke the same polylines.
use super::BrowserSurface;
use crate::theme::Theme;
use gpui::{
    AnyElement, Bounds, Context, MouseButton, Pixels, Point, RenderImage, Window, div, prelude::*,
    px,
};
use std::{cell::Cell, rc::Rc, sync::Arc};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Tool {
    Region,
    Pen,
    Box,
    Arrow,
}

/// Annotation inks: vivid on light and dark pages alike.
const COLORS: [u32; 3] = [0xff3b30, 0xffcc00, 0x0a84ff];
/// Stroke width in logical pixels, so marks read the same at any scale.
const STROKE: f32 = 3.0;

type P = [f32; 2];

#[derive(Clone, Debug, PartialEq)]
struct Mark {
    tool: Tool,
    color: u32,
    width: f32,
    /// Pen: the whole path. Others: start and end.
    points: Vec<P>,
}

pub(super) struct Markup {
    /// BGRA pixels, as GPUI paints them.
    image: Arc<RenderImage>,
    tool: Tool,
    color: u32,
    marks: Vec<Mark>,
    draft: Option<Mark>,
    /// Normalized `[x0, y0, x1, y1]`; `None` keeps the whole capture.
    region: Option<[f32; 4]>,
    /// Where the capture was last painted, for mapping the pointer.
    fit: Rc<Cell<Bounds<Pixels>>>,
}

impl Markup {
    pub(super) fn new(image: Arc<RenderImage>) -> Self {
        Self {
            image,
            tool: Tool::Region,
            color: COLORS[0],
            marks: Vec::new(),
            draft: None,
            region: None,
            fit: Rc::default(),
        }
    }

    pub(super) fn image(&self) -> Arc<RenderImage> {
        self.image.clone()
    }

    fn size(&self) -> P {
        let size = self.image.size(0);
        [size.width.0 as f32, size.height.0 as f32]
    }

    /// Image pixels per logical pixel on screen.
    fn scale(&self) -> f32 {
        self.size()[0] / f32::from(self.fit.get().size.width).max(1.0)
    }

    fn to_image(&self, p: Point<Pixels>) -> P {
        let fit = self.fit.get();
        let [w, h] = self.size();
        let s = self.scale();
        [
            (f32::from(p.x - fit.origin.x) * s).clamp(0.0, w),
            (f32::from(p.y - fit.origin.y) * s).clamp(0.0, h),
        ]
    }

    fn press(&mut self, p: P) {
        let points = if self.tool == Tool::Pen {
            vec![p]
        } else {
            vec![p, p]
        };
        self.draft = Some(Mark {
            tool: self.tool,
            color: self.color,
            width: STROKE * self.scale(),
            points,
        });
    }

    fn drag(&mut self, p: P) {
        let Some(mark) = &mut self.draft else { return };
        if mark.tool == Tool::Pen {
            // Skip sub-pixel jitter; it only adds vertices.
            if mark.points.last().is_none_or(|q| distance(*q, p) >= 1.0) {
                mark.points.push(p);
            }
        } else {
            mark.points[1] = p;
        }
    }

    fn release(&mut self) {
        let Some(mut mark) = self.draft.take() else {
            return;
        };
        match mark.tool {
            Tool::Region => {
                let [a, b] = [mark.points[0], mark.points[1]];
                // A click (no drag) clears the region back to the full page.
                self.region = (distance(a, b) >= 4.0).then(|| normalize(a, b));
            }
            Tool::Pen => {
                if mark.points.len() == 1 {
                    // A tap leaves a dot: give it a length the stroker can cap.
                    let [x, y] = mark.points[0];
                    mark.points.push([x + 0.01, y + 0.01]);
                }
                self.marks.push(mark);
            }
            Tool::Box | Tool::Arrow => {
                if distance(mark.points[0], mark.points[1]) >= 2.0 {
                    self.marks.push(mark);
                }
            }
        }
    }

    pub(super) fn undo(&mut self) {
        if self.marks.pop().is_none() {
            self.region = None;
        }
    }

    /// Everything export needs, detached from the editor for a background thread.
    pub(super) fn export_job(&self) -> impl FnOnce() -> Result<Vec<u8>, String> + Send + 'static {
        let (image, marks, region) = (self.image.clone(), self.marks.clone(), self.region);
        move || export(&image, &marks, region)
    }
}

fn distance(a: P, b: P) -> f32 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}

fn normalize(a: P, b: P) -> [f32; 4] {
    [
        a[0].min(b[0]),
        a[1].min(b[1]),
        a[0].max(b[0]),
        a[1].max(b[1]),
    ]
}

/// The polylines that draw a mark, shared by the screen and the export.
fn outline(mark: &Mark) -> Vec<Vec<P>> {
    let [a, b] = match mark.points[..] {
        [a, b, ..] if mark.tool != Tool::Pen => [a, b],
        _ => return vec![mark.points.clone()],
    };
    match mark.tool {
        Tool::Box => vec![vec![a, [b[0], a[1]], b, [a[0], b[1]], a]],
        Tool::Arrow => {
            let length = distance(a, b);
            let head = (mark.width * 4.5).min(length * 0.5);
            let angle = (a[1] - b[1]).atan2(a[0] - b[0]);
            let wing = |turn: f32| {
                let t = angle + turn;
                [b[0] + head * t.cos(), b[1] + head * t.sin()]
            };
            vec![vec![a, b], vec![wing(0.5), b, wing(-0.5)]]
        }
        Tool::Pen | Tool::Region => Vec::new(),
    }
}

/// Composite the marks onto the capture, crop to the region, encode PNG.
fn export(
    image: &RenderImage,
    marks: &[Mark],
    region: Option<[f32; 4]>,
) -> Result<Vec<u8>, String> {
    let size = image.size(0);
    let (w, h) = (size.width.0 as u32, size.height.0 as u32);
    let bgra = image.as_bytes(0).ok_or("The capture is empty")?;
    let mut rgba = Vec::with_capacity(bgra.len());
    for px in bgra.chunks_exact(4) {
        // tiny-skia wants premultiplied RGBA.
        let a = u16::from(px[3]);
        let mul = |c: u8| ((u16::from(c) * a + 127) / 255) as u8;
        rgba.extend_from_slice(&[mul(px[2]), mul(px[1]), mul(px[0]), px[3]]);
    }
    let size = tiny_skia::IntSize::from_wh(w, h).ok_or("The capture is empty")?;
    let mut pixmap = tiny_skia::Pixmap::from_vec(rgba, size).ok_or("The capture is malformed")?;
    for mark in marks {
        let mut paint = tiny_skia::Paint::default();
        let c = mark.color;
        paint.set_color_rgba8((c >> 16) as u8, (c >> 8) as u8, c as u8, 255);
        paint.anti_alias = true;
        let stroke = tiny_skia::Stroke {
            width: mark.width,
            line_cap: tiny_skia::LineCap::Round,
            line_join: tiny_skia::LineJoin::Round,
            ..Default::default()
        };
        for line in outline(mark) {
            let mut path = tiny_skia::PathBuilder::new();
            for (i, [x, y]) in line.into_iter().enumerate() {
                if i == 0 {
                    path.move_to(x, y)
                } else {
                    path.line_to(x, y)
                }
            }
            if let Some(path) = path.finish() {
                pixmap.stroke_path(
                    &path,
                    &paint,
                    &stroke,
                    tiny_skia::Transform::identity(),
                    None,
                );
            }
        }
    }
    if let Some([x0, y0, x1, y1]) = region {
        let rect = tiny_skia::IntRect::from_ltrb(
            x0.floor() as i32,
            y0.floor() as i32,
            (x1.ceil() as i32).min(w as i32),
            (y1.ceil() as i32).min(h as i32),
        )
        .ok_or("The selected region is empty")?;
        pixmap = pixmap
            .clone_rect(rect)
            .ok_or("The selected region is empty")?;
    }
    pixmap.encode_png().map_err(|e| e.to_string())
}

/// Copy GPUI's BGRA frame so the editor owns a capture that cannot change.
#[cfg(target_os = "linux")]
pub(super) fn freeze(frame: &RenderImage) -> Option<Arc<RenderImage>> {
    let size = frame.size(0);
    let bytes = frame.as_bytes(0)?.to_vec();
    let buffer = image::RgbaImage::from_raw(size.width.0 as u32, size.height.0 as u32, bytes)?;
    Some(Arc::new(RenderImage::new([image::Frame::new(buffer)])))
}

/// Decode a platform capture (TIFF, PNG, ...) into GPUI's BGRA layout.
#[cfg(any(target_os = "macos", windows))]
pub(super) fn decode(bytes: &[u8]) -> Result<Arc<RenderImage>, String> {
    let mut buffer = image::load_from_memory(bytes)
        .map_err(|e| e.to_string())?
        .into_rgba8();
    for px in buffer.chunks_exact_mut(4) {
        px.swap(0, 2);
    }
    Ok(Arc::new(RenderImage::new([image::Frame::new(buffer)])))
}

/// Saved screenshots go to `~/Downloads/Zuse Screenshots`.
pub(super) fn write(bytes: &[u8]) -> Result<std::path::PathBuf, String> {
    use std::io::Write;
    let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
        .ok_or("Home directory unavailable")?;
    let dir = std::path::PathBuf::from(home)
        .join("Downloads")
        .join("Zuse Screenshots");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_nanos();
    let path = dir.join(format!("screenshot-{now}.png"));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|e| e.to_string())?;
    file.write_all(bytes).map_err(|e| e.to_string())?;
    Ok(path)
}

fn contain(bounds: Bounds<Pixels>, [w, h]: P) -> Bounds<Pixels> {
    let s = (f32::from(bounds.size.width) / w).min(f32::from(bounds.size.height) / h);
    let size = gpui::size(px(w * s), px(h * s));
    Bounds::new(
        bounds.center() - gpui::point(size.width / 2.0, size.height / 2.0),
        size,
    )
}

fn paint_mark(mark: &Mark, fit: Bounds<Pixels>, scale: f32, window: &mut Window) {
    let screen = |[x, y]: P| fit.origin + gpui::point(px(x / scale), px(y / scale));
    let width = mark.width / scale;
    for line in outline(mark) {
        let options = gpui::StrokeOptions::default()
            .with_line_width(width)
            .with_line_cap(lyon::tessellation::LineCap::Round)
            .with_line_join(lyon::tessellation::LineJoin::Round);
        let mut path =
            gpui::PathBuilder::stroke(px(width)).with_style(gpui::PathStyle::Stroke(options));
        for (i, p) in line.into_iter().enumerate() {
            if i == 0 {
                path.move_to(screen(p))
            } else {
                path.line_to(screen(p))
            }
        }
        if let Ok(path) = path.build() {
            window.paint_path(path, gpui::rgb(mark.color));
        }
    }
}

impl BrowserSurface {
    /// The editor replaces the page body: the frozen capture, the marks, and a
    /// floating tool bar.
    pub(super) fn markup_editor(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let Some(markup) = &self.markup else {
            return div().into_any_element();
        };
        let image = markup.image.clone();
        let size = markup.size();
        let fit = markup.fit.clone();
        let marks: Vec<Mark> = markup.marks.iter().chain(&markup.draft).cloned().collect();
        let region = match &markup.draft {
            Some(m) if m.tool == Tool::Region => Some(normalize(m.points[0], m.points[1])),
            _ => markup.region,
        };
        let canvas = gpui::canvas(
            |_, _, _| (),
            move |bounds, _, window, _| {
                let rect = contain(bounds, size);
                fit.set(rect);
                let scale = size[0] / f32::from(rect.size.width).max(1.0);
                let _ = window.paint_image(rect, Default::default(), image, 0, false);
                for mark in &marks {
                    paint_mark(mark, rect, scale, window);
                }
                if let Some([x0, y0, x1, y1]) = region {
                    let at =
                        |x: f32, y: f32| rect.origin + gpui::point(px(x / scale), px(y / scale));
                    let (a, b) = (at(x0, y0), at(x1, y1));
                    let shade = gpui::black().opacity(0.5);
                    let (l, t, r, btm) = (rect.left(), rect.top(), rect.right(), rect.bottom());
                    for (o, c) in [
                        (gpui::point(l, t), gpui::point(r, a.y)),
                        (gpui::point(l, b.y), gpui::point(r, btm)),
                        (gpui::point(l, a.y), gpui::point(a.x, b.y)),
                        (gpui::point(b.x, a.y), gpui::point(r, b.y)),
                    ] {
                        window.paint_quad(gpui::fill(Bounds::from_corners(o, c), shade));
                    }
                    window.paint_quad(gpui::outline(
                        Bounds::from_corners(a, b),
                        gpui::white(),
                        gpui::BorderStyle::Solid,
                    ));
                }
            },
        )
        .absolute()
        .inset_0();

        let tool = |id: &'static str, kind: Tool, label: &'static str, glyph: AnyElement| {
            let active = markup.tool == kind;
            div()
                .id(id)
                .debug_selector(move || id.into())
                .size(px(28.0))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(7.0))
                .cursor_pointer()
                .when(active, |el| el.bg(theme.accent_strong))
                .when(!active, |el| el.hover(|el| el.bg(crate::theme::wash(0.1))))
                .aria_label(label)
                .child(glyph)
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(markup) = &mut this.markup {
                        markup.tool = kind;
                        cx.notify();
                    }
                }))
        };
        // Svg icons need their own colour; they do not inherit the button's.
        let glyph = |path: &'static str, active: bool| {
            crate::icons::icon(path)
                .size(px(15.0))
                .text_color(if active {
                    theme.on_accent
                } else {
                    theme.text_muted
                })
                .into_any_element()
        };
        let boxed = |active: bool| {
            div()
                .size(px(12.0))
                .rounded(px(2.0))
                .border_2()
                .border_color(if active {
                    theme.on_accent
                } else {
                    theme.text_muted
                })
                .into_any_element()
        };
        let divider = || div().w(px(1.0)).h(px(18.0)).mx(px(4.0)).bg(theme.border);
        let is = |kind: Tool| markup.tool == kind;

        let swatches = COLORS.map(|color| {
            let active = markup.color == color;
            div()
                .id(("browser-markup-color", color as usize))
                .size(px(28.0))
                .flex()
                .items_center()
                .justify_center()
                .rounded_full()
                .cursor_pointer()
                .hover(|el| el.bg(crate::theme::wash(0.1)))
                .aria_label("Ink colour")
                .child(
                    div()
                        .size(px(14.0))
                        .rounded_full()
                        .bg(gpui::rgb(color))
                        .when(active, |el| el.border_2().border_color(theme.text_strong)),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(markup) = &mut this.markup {
                        markup.color = color;
                        cx.notify();
                    }
                }))
        });

        let bar = crate::popover::popover_card(&theme.for_popup())
            .id("browser-markup-tools")
            .occlude()
            .flex()
            .items_center()
            .gap(px(2.0))
            .p(px(6.0))
            .child(tool(
                "browser-markup-region",
                Tool::Region,
                "Select region",
                glyph(crate::icons::BROWSER_SELECT, is(Tool::Region)),
            ))
            .child(tool(
                "browser-markup-pen",
                Tool::Pen,
                "Pen",
                glyph(crate::icons::PEN, is(Tool::Pen)),
            ))
            .child(tool(
                "browser-markup-box",
                Tool::Box,
                "Box",
                boxed(is(Tool::Box)),
            ))
            .child(tool(
                "browser-markup-arrow",
                Tool::Arrow,
                "Arrow",
                glyph(crate::icons::ARROW_UP_RIGHT, is(Tool::Arrow)),
            ))
            .child(divider())
            .children(swatches)
            .child(divider())
            .child(
                div()
                    .id("browser-markup-undo")
                    .debug_selector(|| "browser-markup-undo".into())
                    .size(px(28.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(7.0))
                    .cursor_pointer()
                    .hover(|el| el.bg(crate::theme::wash(0.1)))
                    .aria_label("Undo")
                    .child(glyph(crate::icons::RESTORE, false))
                    .on_click(cx.listener(|this, _, _, cx| {
                        if let Some(markup) = &mut this.markup {
                            markup.undo();
                            cx.notify();
                        }
                    })),
            )
            .child(div().w(px(6.0)))
            .child(
                super::view::action_button(theme, "browser-markup-cancel", "Cancel", false)
                    .on_click(cx.listener(|this, _, window, cx| this.close_markup(window, cx))),
            )
            .child(div().w(px(6.0)))
            .child(
                super::view::action_button(theme, "browser-markup-save", "Save", true)
                    .on_click(cx.listener(|this, _, window, cx| this.save_markup(window, cx))),
            );

        div()
            .id("browser-markup")
            .debug_selector(|| "browser-markup".into())
            .absolute()
            .inset_0()
            .bg(theme.bg)
            .cursor_crosshair()
            .child(canvas)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &gpui::MouseDownEvent, _, cx| {
                    if let Some(markup) = &mut this.markup {
                        let p = markup.to_image(event.position);
                        markup.press(p);
                        cx.notify();
                    }
                    cx.stop_propagation();
                }),
            )
            .on_mouse_move(cx.listener(|this, event: &gpui::MouseMoveEvent, _, cx| {
                if let Some(markup) = &mut this.markup
                    && markup.draft.is_some()
                {
                    let p = markup.to_image(event.position);
                    markup.drag(p);
                    cx.notify();
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| this.markup_release(cx)),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| this.markup_release(cx)),
            )
            .child(
                div()
                    .absolute()
                    .top(px(10.0))
                    .left_0()
                    .right_0()
                    .flex()
                    .justify_center()
                    .child(bar.cursor_default()),
            )
            .into_any_element()
    }

    fn markup_release(&mut self, cx: &mut Context<Self>) {
        if let Some(markup) = &mut self.markup
            && markup.draft.is_some()
        {
            markup.release();
            cx.notify();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn capture(w: u32, h: u32) -> Arc<RenderImage> {
        // Opaque white BGRA.
        let buffer = image::RgbaImage::from_pixel(w, h, image::Rgba([255, 255, 255, 255]));
        Arc::new(RenderImage::new([image::Frame::new(buffer)]))
    }

    fn editor(w: u32, h: u32) -> Markup {
        let markup = Markup::new(capture(w, h));
        // Painted at half size: two image pixels per logical pixel.
        markup.fit.set(Bounds::new(
            gpui::point(px(10.0), px(20.0)),
            gpui::size(px(w as f32 / 2.0), px(h as f32 / 2.0)),
        ));
        markup
    }

    fn drag(markup: &mut Markup, tool: Tool, from: P, to: P) {
        markup.tool = tool;
        markup.press(from);
        markup.drag(to);
        markup.release();
    }

    #[test]
    fn pointer_maps_to_image_pixels_and_clamps_to_the_capture() {
        let markup = editor(200, 100);
        assert_eq!(
            markup.to_image(gpui::point(px(20.0), px(30.0))),
            [20.0, 20.0]
        );
        assert_eq!(
            markup.to_image(gpui::point(px(0.0), px(500.0))),
            [0.0, 100.0]
        );
        // Strokes keep their on-screen width in image pixels.
        let mut markup = markup;
        markup.press([1.0, 1.0]);
        assert_eq!(markup.draft.as_ref().unwrap().width, STROKE * 2.0);
    }

    #[test]
    fn a_dragged_region_normalizes_and_a_click_clears_it() {
        let mut markup = editor(200, 100);
        drag(&mut markup, Tool::Region, [150.0, 80.0], [50.0, 10.0]);
        assert_eq!(markup.region, Some([50.0, 10.0, 150.0, 80.0]));
        drag(&mut markup, Tool::Region, [5.0, 5.0], [6.0, 6.0]);
        assert_eq!(markup.region, None);
    }

    #[test]
    fn degenerate_shapes_are_dropped_and_undo_unwinds_marks_then_region() {
        let mut markup = editor(200, 100);
        drag(&mut markup, Tool::Region, [10.0, 10.0], [100.0, 90.0]);
        drag(&mut markup, Tool::Box, [10.0, 10.0], [11.0, 10.0]);
        assert!(markup.marks.is_empty());
        drag(&mut markup, Tool::Arrow, [10.0, 10.0], [60.0, 60.0]);
        markup.tool = Tool::Pen;
        markup.press([30.0, 30.0]);
        markup.release();
        assert_eq!(markup.marks.len(), 2);
        assert_eq!(markup.marks[1].points.len(), 2, "a tap becomes a dot");
        markup.undo();
        markup.undo();
        assert!(markup.marks.is_empty());
        assert!(markup.region.is_some());
        markup.undo();
        assert_eq!(markup.region, None);
    }

    #[test]
    fn boxes_close_and_arrows_get_a_head_at_the_end() {
        let mark = |tool| Mark {
            tool,
            color: 0,
            width: 4.0,
            points: vec![[0.0, 0.0], [100.0, 0.0]],
        };
        let lines = outline(&mark(Tool::Box));
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].first(), lines[0].last());
        let lines = outline(&mark(Tool::Arrow));
        assert_eq!(lines[0], vec![[0.0, 0.0], [100.0, 0.0]]);
        let [l, tip, r] = lines[1][..] else {
            panic!("arrow head")
        };
        assert_eq!(tip, [100.0, 0.0]);
        assert!(
            l[0] < 100.0 && r[0] < 100.0 && l[1] * r[1] < 0.0,
            "wings point back"
        );
    }

    #[test]
    fn export_draws_marks_and_crops_to_the_region() {
        let mut markup = editor(200, 100);
        markup.color = 0xff0000;
        drag(&mut markup, Tool::Box, [20.0, 20.0], [180.0, 80.0]);
        drag(&mut markup, Tool::Region, [10.0, 10.0], [110.0, 60.0]);
        let png = markup.export_job()().unwrap();
        let image = image::load_from_memory(&png).unwrap().into_rgba8();
        assert_eq!(image.dimensions(), (100, 50));
        // The box's top edge at y=20 is now y=10; its interior stays white.
        assert_eq!(image.get_pixel(50, 10).0, [255, 0, 0, 255]);
        assert_eq!(image.get_pixel(50, 30).0, [255, 255, 255, 255]);
    }

    #[test]
    fn export_without_region_keeps_the_whole_capture() {
        let markup = editor(64, 32);
        let png = markup.export_job()().unwrap();
        let image = image::load_from_memory(&png).unwrap();
        assert_eq!((image.width(), image.height()), (64, 32));
    }

    #[gpui::test]
    fn the_editor_draws_with_the_pointer_and_keys_undo_and_close(cx: &mut gpui::TestAppContext) {
        use gpui::{Modifiers, MouseButton::Left, point};
        cx.update(|cx| {
            gpui_base::init(cx);
            crate::composer::init(cx, Default::default());
            cx.set_global(crate::theme::Theme::default());
        });
        let (browser, cx) = cx.add_window_view(|window, cx| {
            BrowserSurface::new(super::super::BrowserContext::default(), false, window, cx)
        });
        cx.update(|window, cx| {
            browser.update(cx, |browser, cx| {
                browser.markup = Some(Markup::new(capture(400, 300)));
                window.focus(&browser.focus, cx);
                cx.notify();
            })
        });
        cx.update(|window, cx| window.draw(cx).clear());
        let page = cx.debug_bounds("browser-markup").unwrap();
        let stroke = |cx: &mut gpui::VisualTestContext, from: Point<Pixels>, to: Point<Pixels>| {
            cx.simulate_mouse_down(from, Left, Modifiers::default());
            cx.simulate_mouse_move(to, Some(Left), Modifiers::default());
            cx.simulate_mouse_up(to, Left, Modifiers::default());
        };
        let at = |x: f32, y: f32| page.center() + point(px(x), px(y));
        let read = |cx: &mut gpui::VisualTestContext| {
            browser.read_with(cx, |b, _| {
                b.markup
                    .as_ref()
                    .map(|m| (m.marks.len(), m.region.is_some()))
            })
        };

        // Region is the default tool.
        stroke(cx, at(-60.0, -40.0), at(60.0, 40.0));
        assert_eq!(read(cx), Some((0, true)));

        // Choosing a tool on the bar must not leave a mark on the page under it.
        let pen = cx.debug_bounds("browser-markup-pen").unwrap().center();
        cx.simulate_click(pen, Modifiers::default());
        assert_eq!(read(cx), Some((0, true)));
        stroke(cx, at(-20.0, 0.0), at(20.0, 10.0));
        assert_eq!(read(cx), Some((1, true)));

        cx.simulate_keystrokes(if cfg!(target_os = "macos") {
            "cmd-z"
        } else {
            "ctrl-z"
        });
        assert_eq!(read(cx), Some((0, true)));
        cx.simulate_keystrokes("escape");
        assert_eq!(read(cx), None);
    }
}
