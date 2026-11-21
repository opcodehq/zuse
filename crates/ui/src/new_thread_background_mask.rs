//! Paint-time source-alpha feather. Resizing changes only GPU parameters, not
//! the image identity, pixels, atlas entry, or an asynchronous raster job.
use crate::settings::{NewThreadBackgroundAdjustment, WallpaperLook};
use gpui::{
    App, Bounds, Corners, ImageAlphaMask, Pixels, RenderImage, Window, div, point, prelude::*, px,
    size,
};
use std::{cell::Cell, rc::Rc, sync::Arc};

pub(crate) const CUTOUT_REVEAL_OPACITY: f32 = 0.68;

pub(crate) type SurfaceBounds = Rc<Cell<Option<Bounds<Pixels>>>>;

#[derive(Clone, Copy)]
struct FittedGeometry {
    bounds: Bounds<Pixels>,
    width: f32,
    height: f32,
    overflow_x: f32,
    overflow_y: f32,
}

fn fitted_geometry(
    source_width: f32,
    source_height: f32,
    bounds: Bounds<Pixels>,
    adjustment: NewThreadBackgroundAdjustment,
) -> Option<FittedGeometry> {
    let width = f32::from(bounds.size.width);
    let height = f32::from(bounds.size.height);
    if width <= 0.0
        || height <= 0.0
        || !width.is_finite()
        || !height.is_finite()
        || source_width <= 0.0
        || source_height <= 0.0
        || !source_width.is_finite()
        || !source_height.is_finite()
    {
        return None;
    }

    let adjustment = adjustment.normalized();
    if let Some([left, top, crop_width, crop_height]) = adjustment.crop {
        let scale =
            (width / (source_width * crop_width)).max(height / (source_height * crop_height));
        let fitted_width = source_width * scale;
        let fitted_height = source_height * scale;
        let overflow_x = (fitted_width - width).max(0.0);
        let overflow_y = (fitted_height - height).max(0.0);
        return Some(FittedGeometry {
            bounds: Bounds::new(
                point(
                    bounds.left()
                        - px(((left + crop_width / 2.0) * fitted_width - width / 2.0)
                            .clamp(0.0, overflow_x)),
                    bounds.top()
                        - px(((top + crop_height / 2.0) * fitted_height - height / 2.0)
                            .clamp(0.0, overflow_y)),
                ),
                size(px(fitted_width), px(fitted_height)),
            ),
            width: fitted_width,
            height: fitted_height,
            overflow_x,
            overflow_y,
        });
    }
    let cover = (width / source_width).max(height / source_height);
    let fitted_width = (source_width * cover * adjustment.zoom).max(width);
    let fitted_height = (source_height * cover * adjustment.zoom).max(height);
    let overflow_x = (fitted_width - width).max(0.0);
    let overflow_y = (fitted_height - height).max(0.0);
    let fitted = Bounds::new(
        point(
            bounds.left() - px(overflow_x * adjustment.focal_x),
            bounds.top() - px(overflow_y * adjustment.focal_y),
        ),
        size(px(fitted_width), px(fitted_height)),
    );
    Some(FittedGeometry {
        bounds: fitted,
        width: fitted_width,
        height: fitted_height,
        overflow_x,
        overflow_y,
    })
}

fn source_geometry(
    source: &RenderImage,
    bounds: Bounds<Pixels>,
    adjustment: NewThreadBackgroundAdjustment,
) -> Option<FittedGeometry> {
    let source_size = source.size(0);
    fitted_geometry(
        source_size.width.0 as f32,
        source_size.height.0 as f32,
        bounds,
        adjustment,
    )
}

/// The region of the image the hero shows for `adjustment`, as a normalized
/// `[x, y, width, height]` rect in image space. The inverse of
/// [`adjustment_for_crop`]; both follow [`fitted_geometry`] exactly.
pub(crate) fn crop_for_adjustment(
    image: (f32, f32),
    hero: (f32, f32),
    adjustment: NewThreadBackgroundAdjustment,
) -> [f32; 4] {
    let hero_bounds = Bounds::new(point(px(0.0), px(0.0)), size(px(hero.0), px(hero.1)));
    let adjustment = adjustment.normalized();
    if let Some(crop) = adjustment.crop {
        return crop;
    }
    let Some(geometry) = fitted_geometry(image.0, image.1, hero_bounds, adjustment) else {
        return [0.0, 0.0, 1.0, 1.0];
    };
    [
        geometry.overflow_x * adjustment.focal_x / geometry.width,
        geometry.overflow_y * adjustment.focal_y / geometry.height,
        hero.0 / geometry.width,
        hero.1 / geometry.height,
    ]
}

/// The crop's normalized size at `zoom`: the hero's share of the covered image.
#[cfg(test)]
pub(crate) fn crop_size(image: (f32, f32), hero: (f32, f32), zoom: f32) -> [f32; 2] {
    let (image_w, image_h) = (image.0.max(1.0), image.1.max(1.0));
    let (hero_w, hero_h) = (hero.0.max(1.0), hero.1.max(1.0));
    let cover = (hero_w / image_w).max(hero_h / image_h) * zoom;
    [
        (hero_w / (image_w * cover)).min(1.0),
        (hero_h / (image_h * cover)).min(1.0),
    ]
}

/// The framing that shows the crop whose top-left is `(x, y)` and whose
/// normalized width is `width` (height follows the hero's aspect).
#[cfg(test)]
pub(crate) fn adjustment_for_crop(
    image: (f32, f32),
    hero: (f32, f32),
    x: f32,
    y: f32,
    width: f32,
) -> NewThreadBackgroundAdjustment {
    let [full_width, _] = crop_size(image, hero, 1.0);
    let zoom = full_width / width.max(f32::EPSILON);
    let [width, height] = crop_size(
        image,
        hero,
        zoom.clamp(
            NewThreadBackgroundAdjustment::MIN_ZOOM,
            NewThreadBackgroundAdjustment::MAX_ZOOM,
        ),
    );
    let focal = |origin: f32, size: f32| {
        if 1.0 - size > 1e-4 {
            origin / (1.0 - size)
        } else {
            0.5
        }
    };
    NewThreadBackgroundAdjustment {
        focal_x: focal(x, width),
        focal_y: focal(y, height),
        zoom,
        ..Default::default()
    }
    .normalized()
}

fn mask(
    bounds: Bounds<Pixels>,
    composer: Bounds<Pixels>,
    cutout: bool,
    look: WallpaperLook,
) -> ImageAlphaMask {
    let height = f32::from(bounds.size.height);
    // Keep the cleared area open through the hero's bottom. A taller image
    // must not fade back in beneath the composer's rounded lower edge.
    let cleared = Bounds::new(
        composer.origin,
        size(
            composer.size.width,
            composer.bottom().max(bounds.bottom()) - composer.top(),
        ),
    );
    ImageAlphaMask {
        // The reveal pass has only the shared bottom fade. Its exclusion sits
        // below the image, so it fills the cutout without changing its shape.
        bounds: if cutout {
            cleared
        } else {
            Bounds::new(point(bounds.left(), bounds.bottom() + px(1.0)), bounds.size)
        },
        radius: if cutout {
            px(crate::composer::COMPOSER_RADIUS)
        } else {
            px(0.0)
        },
        feather: if cutout {
            px((height * f32::from(look.soft_edge) / 100.0).clamp(1.0, 180.0))
        } else {
            px(1.0)
        },
        clearance: if cutout { px(3.0) } else { px(0.0) },
        bottom_fade: (look.bottom_fade > 0).then(|| {
            (
                bounds.bottom(),
                px((height * f32::from(look.bottom_fade) / 100.0).max(1.0)),
            )
        }),
    }
}

fn adjusted_image_paint(
    source: &RenderImage,
    bounds: Bounds<Pixels>,
    adjustment: NewThreadBackgroundAdjustment,
    corner_radii: Corners<Pixels>,
) -> Option<(Bounds<Pixels>, Bounds<Pixels>, Corners<Pixels>)> {
    let fitted = source_geometry(source, bounds, adjustment)?;
    Some((bounds, fitted.bounds, corner_radii))
}

fn paint_adjusted_with_mask(
    source: Arc<RenderImage>,
    bounds: Bounds<Pixels>,
    adjustment: NewThreadBackgroundAdjustment,
    corner_radii: Corners<Pixels>,
    mask: Option<ImageAlphaMask>,
    window: &mut Window,
    frame: usize,
) {
    let Some((visible, fitted, corner_radii)) =
        adjusted_image_paint(&source, bounds, adjustment, corner_radii)
    else {
        return;
    };
    let _ =
        window.paint_image_fitted_masked(visible, fitted, corner_radii, source, frame, false, mask);
}

/// The frame of an animated wallpaper due now, on one clock shared by every
/// painter so the hero and its previews stay in step. Stills, and reduced
/// motion, always show the first frame.
#[derive(Clone, Copy, Default)]
pub(crate) struct AnimationFrame {
    pub index: usize,
    next: Option<std::time::Instant>,
}

pub(crate) fn animation_frame(
    source: &RenderImage,
    animate: bool,
    now: std::time::Instant,
) -> AnimationFrame {
    let count = source.frame_count();
    if count <= 1 || !animate {
        return AnimationFrame::default();
    }
    let (index, wait) = frame_and_wait(frame_delays(source), clock_ms(now));
    AnimationFrame {
        index,
        next: Some(now + std::time::Duration::from_millis(wait)),
    }
}

impl AnimationFrame {
    fn request_next(self, window: &mut Window, cx: &App) {
        if let Some(due) = self.next {
            wake_at_next_frame(window, cx, due);
        }
    }
}

/// Each frame's display time in milliseconds.
pub(crate) fn frame_delays(source: &RenderImage) -> impl Iterator<Item = u64> + Clone + '_ {
    (0..source.frame_count()).map(|index| {
        let (numer, denom) = source.delay(index).numer_denom_ms();
        u64::from(numer) / u64::from(denom.max(1))
    })
}

/// Milliseconds on the clock every wallpaper animation shares.
fn clock_ms(now: std::time::Instant) -> u64 {
    static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    now.saturating_duration_since(*START.get_or_init(|| now))
        .as_millis() as u64
}

/// Redraw `window` once when the next frame is due, rather than every
/// display frame: a GIF frame often holds for 100ms or more. Several
/// painters in one window share a single pending wake-up.
fn wake_at_next_frame(window: &mut Window, cx: &App, due: std::time::Instant) {
    use std::{cell::RefCell, collections::HashMap, time::Duration, time::Instant};
    thread_local! {
        static PENDING: RefCell<HashMap<gpui::WindowId, Instant>> = RefCell::default();
    }
    let id = window.window_handle().window_id();
    let now = Instant::now();
    let due = due.max(now + Duration::from_millis(1));
    let scheduled = PENDING.with_borrow_mut(|pending| {
        let earlier = pending.get(&id).is_some_and(|at| *at > now && *at <= due);
        if !earlier {
            pending.insert(id, due);
        }
        earlier
    });
    if scheduled {
        return;
    }
    window
        .spawn(cx, async move |cx| {
            cx.background_executor()
                .timer(due.saturating_duration_since(Instant::now()))
                .await;
            let _ = cx.update(|window, _| window.refresh());
        })
        .detach();
}

/// Which frame of a looping animation shows `elapsed_ms` after it started.
#[cfg(test)]
fn frame_at(delays_ms: impl Iterator<Item = u64> + Clone, elapsed_ms: u64) -> usize {
    frame_and_wait(delays_ms, elapsed_ms).0
}

/// The frame showing at `elapsed_ms`, and how long until the next one.
fn frame_and_wait(delays_ms: impl Iterator<Item = u64> + Clone, elapsed_ms: u64) -> (usize, u64) {
    let total = delays_ms.clone().sum::<u64>().max(1);
    let mut at = elapsed_ms % total;
    for (index, delay) in delays_ms.enumerate() {
        if at < delay {
            return (index, delay - at);
        }
        at -= delay;
    }
    (0, 1)
}

/// Paint the exact responsive crop used by the new-thread hero, without its
/// composer mask. Appearance uses this for a faithful adjustment preview.
pub(crate) fn paint_adjusted(
    source: Arc<RenderImage>,
    bounds: Bounds<Pixels>,
    adjustment: NewThreadBackgroundAdjustment,
    corner_radii: Corners<Pixels>,
    window: &mut Window,
    cx: &gpui::App,
) {
    let frame = animation_frame(&source, !cx.reduce_motion(), std::time::Instant::now());
    frame.request_next(window, cx);
    paint_adjusted_with_mask(
        source,
        bounds,
        adjustment,
        corner_radii,
        None,
        window,
        frame.index,
    );
}

/// All elements have finished prepaint before this reads the measured surface,
/// so the first visible frame uses the current composer, including on sidebar
/// resize and right-panel handoffs. Object-fit cropping is independent.
#[allow(clippy::too_many_arguments)]
pub(crate) fn paint(
    source: Arc<RenderImage>,
    bounds: Bounds<Pixels>,
    composer: Bounds<Pixels>,
    adjustment: NewThreadBackgroundAdjustment,
    cutout: bool,
    look: WallpaperLook,
    window: &mut Window,
    cx: &gpui::App,
) {
    let frame = animation_frame(&source, !cx.reduce_motion(), std::time::Instant::now());
    frame.request_next(window, cx);
    paint_adjusted_with_mask(
        source,
        bounds,
        adjustment,
        Default::default(),
        Some(mask(bounds, composer, cutout, look)),
        window,
        frame.index,
    );
}

pub(crate) fn artwork(
    image: Arc<RenderImage>,
    adjustment: NewThreadBackgroundAdjustment,
    look: WallpaperLook,
    composer_bounds: SurfaceBounds,
    opacity: f32,
    selected_frame: Option<AnimationFrame>,
) -> gpui::Div {
    let shared_frame = Rc::new(Cell::new(AnimationFrame::default()));
    div()
        .size_full()
        .relative()
        .opacity(opacity * f32::from(look.opacity) / 100.0)
        .children([false, true].into_iter().map(|cutout| {
            let image = image.clone();
            let select_image = image.clone();
            let composer_bounds = composer_bounds.clone();
            let prepaint_frame = shared_frame.clone();
            let paint_frame = shared_frame.clone();
            div()
                .absolute()
                .inset_0()
                .opacity(if cutout { 1.0 } else { CUTOUT_REVEAL_OPACITY })
                .child(
                    gpui::canvas(
                        move |_, _, cx| {
                            if !cutout {
                                prepaint_frame.set(selected_frame.unwrap_or_else(|| {
                                    animation_frame(
                                        &select_image,
                                        !cx.reduce_motion(),
                                        std::time::Instant::now(),
                                    )
                                }));
                            }
                        },
                        move |bounds, _, window, cx| {
                            let composer = composer_bounds.get().unwrap_or_else(|| {
                                Bounds::new(
                                    point(bounds.left(), bounds.bottom() + px(200.0)),
                                    bounds.size,
                                )
                            });
                            let frame = paint_frame.get();
                            if !cutout {
                                frame.request_next(window, cx);
                            }
                            paint_adjusted_with_mask(
                                image.clone(),
                                bounds,
                                adjustment,
                                Default::default(),
                                Some(mask(bounds, composer, cutout, look)),
                                window,
                                frame.index,
                            );
                        },
                    )
                    .absolute()
                    .inset_0(),
                )
        }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crop_rect_and_framing_round_trip_through_the_renderer() {
        let (image, hero) = ((1600.0, 900.0), (1180.0, 760.0));
        for adjustment in [
            NewThreadBackgroundAdjustment::default(),
            NewThreadBackgroundAdjustment {
                focal_x: 0.2,
                focal_y: 0.9,
                zoom: 2.5,
                ..Default::default()
            },
        ] {
            let [x, y, width, height] = crop_for_adjustment(image, hero, adjustment);
            // The crop is exactly what the hero paints.
            let geometry = fitted_geometry(
                image.0,
                image.1,
                Bounds::new(point(px(0.0), px(0.0)), size(px(hero.0), px(hero.1))),
                adjustment,
            )
            .unwrap();
            assert!((width - hero.0 / geometry.width).abs() < 1e-4);
            assert!((height - hero.1 / geometry.height).abs() < 1e-4);
            assert!((x - geometry.overflow_x * adjustment.focal_x / geometry.width).abs() < 1e-4);
            let back = adjustment_for_crop(image, hero, x, y, width);
            assert!((back.zoom - adjustment.zoom).abs() < 1e-3);
            assert!((back.focal_x - adjustment.focal_x).abs() < 1e-3);
            assert!((back.focal_y - adjustment.focal_y).abs() < 1e-3);
        }
        // Wider than allowed clamps to the full cover; tiny clamps to max zoom.
        assert_eq!(adjustment_for_crop(image, hero, 0.0, 0.0, 5.0).zoom, 1.0);
        assert_eq!(
            adjustment_for_crop(image, hero, 0.0, 0.0, 0.001).zoom,
            NewThreadBackgroundAdjustment::MAX_ZOOM
        );
    }

    #[test]
    fn animation_loops_through_frames_by_their_delays() {
        let delays = [100u64, 50, 200].into_iter();
        assert_eq!(frame_at(delays.clone(), 0), 0);
        assert_eq!(frame_at(delays.clone(), 99), 0);
        assert_eq!(frame_at(delays.clone(), 100), 1);
        assert_eq!(frame_at(delays.clone(), 151), 2);
        assert_eq!(frame_at(delays.clone(), 350), 0);
        assert_eq!(frame_at(delays.clone(), 350 * 7 + 120), 1);
        // The wake-up lands on the next frame boundary, not every display frame.
        assert_eq!(frame_and_wait(delays.clone(), 30), (0, 70));
        assert_eq!(frame_and_wait(delays, 160), (2, 190));
    }
    use gpui::{Context, Render, canvas, div};

    fn hero(width: f32, height: f32) -> Bounds<Pixels> {
        Bounds::new(point(px(40.0), px(24.0)), size(px(width), px(height)))
    }

    fn fitted(
        source: (f32, f32),
        viewport: Bounds<Pixels>,
        adjustment: NewThreadBackgroundAdjustment,
    ) -> FittedGeometry {
        fitted_geometry(source.0, source.1, viewport, adjustment).unwrap()
    }

    fn assert_near(actual: Pixels, expected: Pixels) {
        assert!(
            (f32::from(actual - expected)).abs() < 0.001,
            "expected {expected:?}, got {actual:?}"
        );
    }

    fn source(width: u32, height: u32) -> RenderImage {
        RenderImage::new([image::Frame::new(image::RgbaImage::from_pixel(
            width,
            height,
            image::Rgba([79, 151, 233, 180]),
        ))])
    }

    #[test]
    fn default_adjustment_is_the_existing_centered_cover_crop() {
        let viewport = hero(1000.0, 500.0);
        let crop = fitted(
            (1600.0, 900.0),
            viewport,
            NewThreadBackgroundAdjustment::default(),
        );
        assert_eq!(crop.bounds.size.width, px(1000.0));
        assert_eq!(crop.bounds.size.height, px(562.5));
        assert_eq!(crop.bounds.left(), viewport.left());
        assert_eq!(crop.bounds.top(), viewport.top() - px(31.25));
        assert_eq!(crop.bounds.center(), viewport.center());
    }

    #[test]
    fn focal_extremes_align_the_overflowing_image_edges() {
        let viewport = hero(1000.0, 500.0);
        let start = fitted(
            (1600.0, 900.0),
            viewport,
            NewThreadBackgroundAdjustment {
                focal_x: 0.0,
                focal_y: 0.0,
                zoom: 2.0,
                ..Default::default()
            },
        );
        assert_eq!(start.bounds.left(), viewport.left());
        assert_eq!(start.bounds.top(), viewport.top());

        let end = fitted(
            (1600.0, 900.0),
            viewport,
            NewThreadBackgroundAdjustment {
                focal_x: 1.0,
                focal_y: 1.0,
                zoom: 2.0,
                ..Default::default()
            },
        );
        assert_near(end.bounds.right(), viewport.right());
        assert_near(end.bounds.bottom(), viewport.bottom());
    }

    #[test]
    fn zoom_multiplies_cover_before_positioning() {
        let viewport = hero(1000.0, 500.0);
        let base = fitted(
            (1600.0, 900.0),
            viewport,
            NewThreadBackgroundAdjustment::default(),
        );
        let zoomed = fitted(
            (1600.0, 900.0),
            viewport,
            NewThreadBackgroundAdjustment {
                zoom: 2.0,
                ..Default::default()
            },
        );
        assert_near(zoomed.bounds.size.width, base.bounds.size.width * 2.0);
        assert_near(zoomed.bounds.size.height, base.bounds.size.height * 2.0);
        assert_eq!(zoomed.bounds.center(), viewport.center());
    }

    #[test]
    fn resizing_preserves_the_normalized_focal_alignment() {
        let adjustment = NewThreadBackgroundAdjustment {
            focal_x: 0.23,
            focal_y: 0.71,
            zoom: 2.2,
            ..Default::default()
        };
        for viewport in [hero(420.0, 300.0), hero(960.0, 420.0), hero(1200.0, 760.0)] {
            let crop = fitted((1600.0, 900.0), viewport, adjustment);
            let x = f32::from(viewport.left() - crop.bounds.left()) / crop.overflow_x;
            let y = f32::from(viewport.top() - crop.bounds.top()) / crop.overflow_y;
            assert!((x - adjustment.focal_x).abs() < 0.0001);
            assert!((y - adjustment.focal_y).abs() < 0.0001);
        }
    }

    #[test]
    fn every_supported_crop_covers_the_entire_viewport() {
        for source in [(900.0, 1600.0), (1600.0, 900.0), (1000.0, 1000.0)] {
            for viewport in [hero(360.0, 760.0), hero(1000.0, 500.0), hero(640.0, 640.0)] {
                for focal_x in [0.0, 0.37, 1.0] {
                    for focal_y in [0.0, 0.61, 1.0] {
                        for zoom in [1.0, 1.8, NewThreadBackgroundAdjustment::MAX_ZOOM] {
                            let crop = fitted(
                                source,
                                viewport,
                                NewThreadBackgroundAdjustment {
                                    focal_x,
                                    focal_y,
                                    zoom,
                                    ..Default::default()
                                },
                            );
                            assert!(crop.bounds.left() <= viewport.left());
                            assert!(crop.bounds.top() <= viewport.top());
                            assert!(crop.bounds.right() >= viewport.right());
                            assert!(crop.bounds.bottom() >= viewport.bottom());
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn masked_passes_share_identical_adjusted_geometry() {
        let viewport = hero(1000.0, 500.0);
        let composer = Bounds::new(point(px(180.0), px(320.0)), size(px(720.0), px(124.0)));
        let adjustment = NewThreadBackgroundAdjustment {
            focal_x: 0.17,
            focal_y: 0.82,
            zoom: 1.7,
            ..Default::default()
        };
        let main = fitted((1600.0, 900.0), viewport, adjustment);
        let reveal = fitted((1600.0, 900.0), viewport, adjustment);
        assert_eq!(main.bounds, reveal.bounds);
        let main_mask = mask(viewport, composer, true, WallpaperLook::default());
        let reveal_mask = mask(viewport, composer, false, WallpaperLook::default());
        assert_ne!(main_mask.bounds, reveal_mask.bounds);
        assert_eq!(main_mask.bottom_fade, reveal_mask.bottom_fade);
    }

    #[test]
    fn preview_paint_keeps_rounded_viewport_corners_when_panning_and_zooming() {
        let source = source(16, 9);
        let viewport = hero(100.0, 50.0);
        let rounded = Corners::all(px(11.0));
        let initial = NewThreadBackgroundAdjustment::default();
        let zoomed = NewThreadBackgroundAdjustment {
            zoom: 2.0,
            ..initial
        };
        let panned = NewThreadBackgroundAdjustment {
            focal_x: 0.3,
            focal_y: 0.7,
            ..zoomed
        };

        for adjustment in [initial, zoomed, panned] {
            let (visible, fitted, corners) =
                adjusted_image_paint(&source, viewport, adjustment, rounded).unwrap();
            let (hero_visible, hero_fitted, hero_corners) =
                adjusted_image_paint(&source, viewport, adjustment, Default::default()).unwrap();
            assert_eq!(visible, viewport);
            assert_eq!(corners, rounded);
            assert_eq!(hero_corners, Corners::all(px(0.0)));
            assert_eq!((visible, fitted), (hero_visible, hero_fitted));
            assert_eq!(
                fitted,
                source_geometry(&source, viewport, adjustment)
                    .unwrap()
                    .bounds
            );
        }
    }

    #[gpui::test]
    fn background_paint_sees_same_frame_composer_bounds_even_when_painted_first(
        cx: &mut gpui::TestAppContext,
    ) {
        struct Fixture {
            surface: SurfaceBounds,
            painted: SurfaceBounds,
            source: Arc<RenderImage>,
            left: f32,
            width: f32,
        }
        impl Render for Fixture {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                let surface = self.surface.clone();
                let painted = self.painted.clone();
                let measured = self.surface.clone();
                let source = self.source.clone();
                div()
                    .relative()
                    .size_full()
                    .child(
                        canvas(
                            |_, _, _| {},
                            move |bounds, _, window, cx| {
                                painted.set(surface.get());
                                if let Some(composer) = surface.get() {
                                    paint(
                                        source,
                                        bounds,
                                        composer,
                                        NewThreadBackgroundAdjustment::default(),
                                        true,
                                        WallpaperLook::default(),
                                        window,
                                        cx,
                                    );
                                }
                            },
                        )
                        .absolute()
                        .inset_0(),
                    )
                    .child(
                        div()
                            .absolute()
                            .left(px(self.left))
                            .top(px(360.25))
                            .w(px(self.width))
                            .h(px(124.0))
                            .child(
                                canvas(
                                    move |bounds, _, _| measured.set(Some(bounds)),
                                    |_, _, _, _| {},
                                )
                                .absolute()
                                .inset_0(),
                            ),
                    )
            }
        }
        let surface: SurfaceBounds = Default::default();
        let painted: SurfaceBounds = Default::default();
        let handle = cx.add_window(|_, _| Fixture {
            surface: surface.clone(),
            painted: painted.clone(),
            source: Arc::new(RenderImage::new([image::Frame::new(
                image::RgbaImage::from_pixel(2, 2, image::Rgba([79, 151, 233, 180])),
            )])),
            left: 40.0,
            width: 768.0,
        });
        for (left, width) in [
            (40.0, 768.0),
            (264.0, 544.0),
            (152.25, 656.0),
            (40.0, 408.0),
            (40.0, 768.0),
        ] {
            handle
                .update(cx, |fixture, _, cx| {
                    fixture.left = left;
                    fixture.width = width;
                    cx.notify();
                })
                .unwrap();
            cx.update_window(handle.into(), |_, window, cx| {
                window.draw(cx).clear();
            })
            .unwrap();
            let actual = painted.get().expect("no cold first-frame geometry");
            assert_eq!(Some(actual), surface.get());
            // GPUI layout snaps to physical pixels; the mask must consume
            // that exact measured surface, not the unrounded layout request.
            assert!((f32::from(actual.left()) - left).abs() <= 0.5);
            assert_eq!(actual.size.width, px(width));
        }
    }

    #[test]
    fn mask_tracks_current_surface_in_window_space_without_rounding() {
        for sidebar in [0.0, 112.25, 224.0] {
            for right_panel in [0.0, 360.0] {
                let hero = Bounds::new(
                    point(px(sidebar), px(40.0)),
                    size(px(1200.0 - sidebar), px(440.0)),
                );
                let composer = Bounds::new(
                    point(px(sidebar + 40.5), px(360.25)),
                    size(px(900.0 - sidebar - right_panel), px(124.0)),
                );
                let mask = mask(hero, composer, true, WallpaperLook::default());
                assert_eq!(mask.bounds, composer);
                assert_eq!(mask.bottom_fade, Some((px(480.0), px(286.0))));
                assert_eq!(mask.feather, px(440.0 * 16.0 / 100.0));
                assert_eq!(mask.clearance, px(3.0));
            }
        }
    }

    #[test]
    fn taller_background_stays_cleared_below_the_composer() {
        let hero = Bounds::new(point(px(0.0), px(0.0)), size(px(1440.0), px(691.2)));
        let composer = Bounds::new(point(px(352.0), px(406.0)), size(px(736.0), px(124.0)));
        let mask = mask(hero, composer, true, WallpaperLook::default());
        assert_eq!(mask.bounds.origin, composer.origin);
        assert_eq!(mask.bounds.size.width, composer.size.width);
        assert_eq!(mask.bounds.bottom(), hero.bottom());
        assert!(mask.feather < px(120.0));
    }
    #[test]
    fn new_thread_cutout_reveal_preserves_the_bottom_fade_and_image_extent() {
        let hero = Bounds::new(point(px(0.0), px(0.0)), size(px(1440.0), px(691.2)));
        let composer = Bounds::new(point(px(352.0), px(406.0)), size(px(736.0), px(124.0)));
        let cutout = mask(hero, composer, true, WallpaperLook::default());
        let reveal = mask(hero, composer, false, WallpaperLook::default());
        assert_eq!(cutout.bottom_fade, reveal.bottom_fade);
        assert!(reveal.bounds.top() - reveal.feather >= hero.bottom());
        assert_eq!(reveal.radius, px(0.0));
        assert_eq!(reveal.clearance, px(0.0));
    }

    #[test]
    fn new_thread_main_fade_uses_the_requested_fraction_at_every_window_size() {
        for height in [288.0, 489.6, 691.2, 760.0] {
            let hero = Bounds::new(point(px(224.25), px(40.5)), size(px(1000.0), px(height)));
            let composer = Bounds::new(point(px(352.0), px(406.0)), size(px(736.0), px(124.0)));
            for cutout in [false, true] {
                let look = WallpaperLook {
                    bottom_fade: 40,
                    ..WallpaperLook::default()
                };
                let (end, feather) = mask(hero, composer, cutout, look).bottom_fade.unwrap();
                assert!((f32::from(feather) - height * 0.4).abs() < 0.0001);
                assert_eq!(end, hero.bottom());
            }
        }
    }

    #[test]
    fn fade_can_be_disabled_without_removing_the_composer_cutout() {
        let viewport = hero(1000.0, 500.0);
        let composer = Bounds::new(point(px(180.0), px(320.0)), size(px(720.0), px(124.0)));
        let look = WallpaperLook {
            bottom_fade: 0,
            soft_edge: 0,
            ..WallpaperLook::default()
        };
        let cutout = mask(viewport, composer, true, look);
        assert_eq!(cutout.bottom_fade, None);
        assert_eq!(cutout.feather, px(1.0));
        assert_eq!(cutout.bounds.origin, composer.origin);
    }
}
