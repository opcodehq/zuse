//! Profile pictures for Zappers and the people working with them.
//!
//! A Zapper is either the user's own picture or one of the flat mascots in
//! [`zapper_mascots`], drawn on a transparent ground as smooth filled
//! outlines. Mascots turn their heads toward the pointer, blink, glance
//! around and show their mood; a change of mood blends over a few hundred
//! milliseconds instead of jumping. A shared sampler re-poses every live
//! mascot thirty times a second and re-renders a view only when one of its
//! mascots would draw differently, so quiet mascots cost no paints between
//! blinks. Mascots that stop being painted (scrolled away, hidden, closed)
//! drop out of the sampler. Under reduced motion mascots hold a still pose
//! that still shows their mood, and moods change without blending. People
//! get a quiet monogram disc; an unassigned slot is a dashed ring.
use base64::Engine as _;
use gpui::{prelude::*, *};
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use zuse_protocol::presentation::zapper_mascots::{self, Animator, Mascot, Mood, Outline, Pose};
use zuse_protocol::zappers::{AVATAR_IMAGE_MAX_LEN, Zapper, ZapperAvatar, avatar_image_ok};

/// Everything needed to draw one Zapper: its mascot, which is also the
/// fallback, and the user's picture when it has one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ZapperFace {
    pub avatar: ZapperAvatar,
    pub image: Option<SharedString>,
    /// What the Zapper is doing, shown by its mascot.
    pub mood: Mood,
}

impl ZapperFace {
    pub fn mascot(avatar: ZapperAvatar) -> Self {
        Self {
            avatar,
            image: None,
            mood: Mood::Awake,
        }
    }

    pub fn of(zapper: &Zapper) -> Self {
        Self {
            avatar: zapper
                .config
                .avatar
                .clone()
                .unwrap_or_else(|| zapper_mascots::default_avatar(&zapper.id)),
            image: zapper.config.avatar_image.clone().map(SharedString::from),
            mood: Mood::Awake,
        }
    }

    pub fn with_mood(self, mood: Mood) -> Self {
        Self { mood, ..self }
    }
}

/// A Zapper's mascot, `size` logical pixels across.
pub fn zapper_avatar(avatar: &ZapperAvatar, size: f32) -> Div {
    zapper_face(&ZapperFace::mascot(avatar.clone()), size)
}

/// A Zapper's picture, `size` logical pixels across: its own image when it
/// has a readable one, otherwise its mascot.
pub fn zapper_face(face: &ZapperFace, size: f32) -> Div {
    let radius = px(size * 0.2);
    if let Some(image) = face.image.as_deref().and_then(decoded) {
        return div().size(px(size)).flex_none().child(
            img(image)
                .size(px(size))
                .rounded(radius)
                .object_fit(ObjectFit::Cover),
        );
    }
    let (mascot, color) = zapper_mascots::resolve(&face.avatar);
    let body = color.body;
    let mood = face.mood;
    div().size(px(size)).flex_none().child(
        canvas(
            |_, _, _| {},
            move |bounds, _, window, cx| {
                let pose = if cx.reduce_motion() {
                    zapper_mascots::still_pose(mood)
                } else {
                    live_pose(mascot, mood, size, bounds, window, cx)
                };
                let shape = zapper_mascots::outline(mascot, pose, zapper_mascots::line_for(size));
                // Offline Zappers fade, so a stale face never reads as present.
                let alpha = if mood == Mood::Offline { 0.5 } else { 1. };
                paint_outline(&shape, body, alpha, bounds, window);
            },
        )
        .size(px(size)),
    )
}

/// Remember the pictures of these Zappers, so surfaces that only know a
/// Zapper's id (a message's sender line) can show its picture too.
pub fn remember_zappers<'a>(zappers: impl IntoIterator<Item = &'a Zapper>) {
    KNOWN.with_borrow_mut(|known| {
        for zapper in zappers {
            match &zapper.config.avatar_image {
                Some(image) => {
                    known.insert(zapper.id.clone(), SharedString::from(image.clone()));
                }
                None => {
                    known.remove(&zapper.id);
                }
            }
        }
    });
}

/// The face for a Zapper known by id, preferring its remembered picture.
pub fn known_face(zapper_id: &str, avatar: &ZapperAvatar) -> ZapperFace {
    ZapperFace {
        avatar: avatar.clone(),
        image: KNOWN.with_borrow(|known| known.get(zapper_id).cloned()),
        mood: Mood::Awake,
    }
}

/// Largest picture accepted before decoding.
const UPLOAD_MAX_BYTES: usize = 25 * 1024 * 1024;
/// Side of the stored square: sharp at 64 logical pixels on a 2x screen.
pub const AVATAR_IMAGE_SIDE: u32 = 128;

/// Turn an uploaded picture into a stored avatar: centre-cropped to a square,
/// scaled down to [`AVATAR_IMAGE_SIDE`], and encoded as a small PNG, or as a
/// JPEG when a photo would make the PNG too large. Returns a `data:` URL.
pub fn prepare_image(bytes: &[u8]) -> Result<String, &'static str> {
    if bytes.len() > UPLOAD_MAX_BYTES {
        return Err("That image is too large. Pick one under 25 MB.");
    }
    let source = image::load_from_memory(bytes)
        .map_err(|_| "Zuse couldn't read that image. Try a PNG or JPEG.")?;
    let (width, height) = (source.width(), source.height());
    let side = width.min(height);
    if side == 0 {
        return Err("That image is empty.");
    }
    let square = source.crop_imm((width - side) / 2, (height - side) / 2, side, side);
    let target = side.min(AVATAR_IMAGE_SIDE);
    let square = if side > target {
        square.resize_exact(target, target, image::imageops::FilterType::Lanczos3)
    } else {
        square
    };
    let url = |mime: &str, data: &[u8]| {
        format!(
            "data:{mime};base64,{}",
            base64::engine::general_purpose::STANDARD.encode(data)
        )
    };
    let mut png = Vec::new();
    square
        .to_rgba8()
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .map_err(|_| "Zuse couldn't prepare that image.")?;
    let png = url("image/png", &png);
    // Pictures with transparency or flat colour stay PNG; photos are far
    // smaller as JPEG.
    if png.len() <= AVATAR_IMAGE_MAX_LEN / 2 {
        return Ok(png);
    }
    let rgb = image::DynamicImage::ImageRgba8(flatten(square.to_rgba8())).to_rgb8();
    for quality in [88, 75, 60, 45] {
        let mut jpeg = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, quality)
            .encode_image(&rgb)
            .map_err(|_| "Zuse couldn't prepare that image.")?;
        let jpeg = url("image/jpeg", &jpeg);
        if avatar_image_ok(&jpeg) {
            return Ok(jpeg);
        }
    }
    if avatar_image_ok(&png) {
        Ok(png)
    } else {
        Err("That image is too detailed to use as a picture.")
    }
}

/// Composite transparent pixels over the dark avatar tile, since JPEG has
/// no alpha.
fn flatten(mut rgba: image::RgbaImage) -> image::RgbaImage {
    for pixel in rgba.pixels_mut() {
        let alpha = f32::from(pixel[3]) / 255.;
        for (channel, back) in pixel.0.iter_mut().take(3).zip([0x14u8, 0x14, 0x14]) {
            *channel = (f32::from(*channel) * alpha + f32::from(back) * (1. - alpha)).round() as u8;
        }
        pixel[3] = 255;
    }
    rgba
}

thread_local! {
    static KNOWN: RefCell<HashMap<String, SharedString>> = RefCell::new(HashMap::new());
    static DECODED: RefCell<HashMap<u64, Option<Arc<Image>>>> = RefCell::new(HashMap::new());
}

/// The picture behind a stored `data:` URL, decoded once per distinct value.
fn decoded(url: &str) -> Option<Arc<Image>> {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    url.hash(&mut hasher);
    let key = hasher.finish();
    DECODED.with_borrow_mut(|cache| {
        if let Some(image) = cache.get(&key) {
            return image.clone();
        }
        // A handful of Zappers is normal; never let edits grow this forever.
        if cache.len() >= 64 {
            cache.clear();
        }
        let image = decode_url(url).map(Arc::new);
        cache.insert(key, image.clone());
        image
    })
}

fn decode_url(url: &str) -> Option<Image> {
    if !avatar_image_ok(url) {
        return None;
    }
    let (format, data) = if let Some(data) = url.strip_prefix("data:image/png;base64,") {
        (ImageFormat::Png, data)
    } else {
        (
            ImageFormat::Jpeg,
            url.strip_prefix("data:image/jpeg;base64,")?,
        )
    };
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data)
        .ok()?;
    Some(Image::from_bytes(format, bytes))
}

/// How far the pointer must be, in logical pixels, for a full turn.
const LOOK_REACH: f32 = 360.;
/// How often live mascots are re-posed: thirty times a second.
const SAMPLE: Duration = Duration::from_millis(33);
/// A view that was asked to repaint and didn't is no longer showing the
/// mascot; forget it after this long.
const GONE_AFTER: Duration = Duration::from_millis(400);
/// Mascots painted this close together belong to the same frame.
const SAME_FRAME: Duration = Duration::from_millis(100);

/// One mascot on screen, keyed by its view and where it sits.
struct Live {
    view: EntityId,
    window: AnyWindowHandle,
    centre: Point<Pixels>,
    mascot: &'static Mascot,
    animator: Animator,
    size: f32,
    scale: f32,
    /// Smoothed look targets: the eyes lead, the head follows.
    eyes: (f32, f32),
    head: (f32, f32),
    sampled: Instant,
    /// When it was last painted.
    painted: Instant,
    drawn: u64,
    asked: Option<Instant>,
}

#[derive(Default)]
struct Motion {
    epoch: Option<Instant>,
    live: HashMap<(EntityId, u64), Live>,
    /// When each view last painted any mascot. A view that paints again
    /// without one of its mascots no longer shows it: quiet mascots never
    /// ask for a repaint, so this is how they leave the sampler.
    views: HashMap<EntityId, Instant>,
    running: bool,
    /// A stand-in pointer for visual fixtures, in window coordinates.
    pointer: Option<Point<Pixels>>,
}

/// Make every mascot look at `at` instead of the real pointer, for fixtures
/// that record the animation without moving anyone's mouse.
#[doc(hidden)]
pub fn fixture_point_at(at: Option<Point<Pixels>>, cx: &mut App) {
    cx.default_global::<Motion>().pointer = at;
}

impl Global for Motion {}

fn now_ms(motion: &mut Motion) -> u64 {
    motion
        .epoch
        .get_or_insert_with(Instant::now)
        .elapsed()
        .as_millis() as u64
}

fn digest(value: impl std::hash::Hash) -> u64 {
    use std::hash::{BuildHasher, BuildHasherDefault};
    BuildHasherDefault::<std::collections::hash_map::DefaultHasher>::default().hash_one(value)
}

fn pose_of(live: &Live, ms: u64) -> Pose {
    live.animator.pose(live.mascot, ms, live.eyes, live.head)
}

/// What a mascot would draw, to a third of a device pixel: equal keys
/// paint identical frames.
fn drawn(live: &Live, ms: u64) -> u64 {
    let shape = zapper_mascots::outline(
        live.mascot,
        pose_of(live, ms),
        zapper_mascots::line_for(live.size),
    );
    let unit = live.size / 2. * live.scale * 3.;
    digest(
        shape
            .body
            .iter()
            .chain(shape.eyes.iter().flatten())
            .map(|p| ((p.0 * unit).round() as i32, (p.1 * unit).round() as i32))
            .collect::<Vec<_>>(),
    )
}

/// Pose a mascot being painted, registering it with the sampler.
fn live_pose(
    mascot: &'static Mascot,
    mood: Mood,
    size: f32,
    bounds: Bounds<Pixels>,
    window: &mut Window,
    cx: &mut App,
) -> Pose {
    let view = window.current_view();
    let centre = bounds.center();
    let slot = digest((
        mascot.id,
        size.to_bits(),
        f32::from(centre.x).round() as i32,
        f32::from(centre.y).round() as i32,
    ));
    let window_handle = window.window_handle();
    let motion = cx.default_global::<Motion>();
    let ms = now_ms(motion);
    let scale = window.scale_factor();
    let live = motion.live.entry((view, slot)).or_insert_with(|| Live {
        view,
        window: window_handle,
        centre,
        mascot,
        animator: Animator::new(mood),
        size,
        scale,
        eyes: (0., 0.),
        head: (0., 0.),
        sampled: Instant::now(),
        painted: Instant::now(),
        drawn: 0,
        asked: None,
    });
    live.asked = None;
    live.painted = Instant::now();
    let painted = live.painted;
    live.scale = scale;
    // A new mood blends in from the pose on screen; the same mood again is
    // a no-op, so re-renders never restart anything.
    live.animator.set(mascot, mood, ms, live.eyes, live.head);
    let pose = pose_of(live, ms);
    live.drawn = drawn(live, ms);
    motion.views.insert(view, painted);
    if !motion.running {
        motion.running = true;
        cx.spawn(async move |cx| {
            loop {
                cx.background_executor().timer(SAMPLE).await;
                if !cx.update(sample) {
                    break;
                }
            }
        })
        .detach();
    }
    pose
}

/// Re-pose every live mascot; ask a view to repaint only when one of its
/// mascots would draw differently. Returns whether to keep sampling.
fn sample(cx: &mut App) -> bool {
    if cx.reduce_motion() {
        // Repaint once more so every mascot settles into its still pose
        // instead of freezing mid-gesture.
        let motion = cx.default_global::<Motion>();
        let mut views: Vec<EntityId> = motion.live.drain().map(|(_, live)| live.view).collect();
        motion.views.clear();
        motion.running = false;
        views.sort_unstable();
        views.dedup();
        for view in views {
            cx.notify(view);
        }
        return false;
    }
    // Where the pointer is in each window holding a mascot.
    let windows: Vec<AnyWindowHandle> = {
        let motion = cx.default_global::<Motion>();
        let mut windows: Vec<_> = motion.live.values().map(|l| l.window).collect();
        windows.dedup();
        windows
    };
    let stand_in = cx.default_global::<Motion>().pointer;
    let mut pointers = HashMap::new();
    for handle in windows {
        if let Some(at) = stand_in {
            pointers.insert(handle.window_id(), Some(Some(at)));
            continue;
        }
        let pointer = handle
            .update(cx, |_, window, _| {
                window.is_window_active().then(|| window.mouse_position())
            })
            .ok();
        pointers.insert(handle.window_id(), pointer);
    }
    let motion = cx.default_global::<Motion>();
    let ms = now_ms(motion);
    let now = Instant::now();
    let mut repaint = Vec::new();
    let views = &motion.views;
    motion.live.retain(|_, live| {
        let Some(pointer) = pointers.get(&live.window.window_id()).copied().flatten() else {
            // The window closed.
            return false;
        };
        if live.asked.is_some_and(|at| now - at > GONE_AFTER) {
            return false;
        }
        if views
            .get(&live.view)
            .is_some_and(|at| *at > live.painted + SAME_FRAME)
        {
            return false;
        }
        let target = match pointer {
            Some(at) => (
                (f32::from(at.x - live.centre.x) / LOOK_REACH).clamp(-1., 1.),
                (f32::from(at.y - live.centre.y) / LOOK_REACH).clamp(-1., 1.),
            ),
            // An inactive window's mascots drift back to facing forward.
            None => (0., 0.),
        };
        let dt = (now - live.sampled).as_secs_f32().min(0.064) * 1000.;
        live.sampled = now;
        let ease = |from: (f32, f32), ms: f32| {
            let k = 1. - (-dt / ms).exp();
            (
                from.0 + (target.0 - from.0) * k,
                from.1 + (target.1 - from.1) * k,
            )
        };
        live.eyes = ease(live.eyes, zapper_mascots::EYES_FOLLOW_MS);
        live.head = ease(live.head, zapper_mascots::HEAD_FOLLOW_MS);
        if drawn(live, ms) != live.drawn && live.asked.is_none() {
            live.asked = Some(now);
            repaint.push(live.view);
        }
        true
    });
    let shown: std::collections::HashSet<EntityId> = motion.live.values().map(|l| l.view).collect();
    motion.views.retain(|view, _| shown.contains(view));
    let keep = !motion.live.is_empty();
    motion.running = keep;
    repaint.sort_unstable();
    repaint.dedup();
    for view in repaint {
        cx.notify(view);
    }
    keep
}

/// Fill a posed mascot's outlines into `bounds`: the body in its colour,
/// then the eyes.
fn paint_outline(
    shape: &Outline,
    body: u32,
    alpha: f32,
    bounds: Bounds<Pixels>,
    window: &mut Window,
) {
    let side = f32::from(bounds.size.width.min(bounds.size.height));
    let origin = bounds.origin;
    let at = |p: &(f32, f32)| {
        point(
            origin.x + px((p.0 + 1.) / 2. * side),
            origin.y + px((p.1 + 1.) / 2. * side),
        )
    };
    let fill_with = |points: &[(f32, f32)], color: u32, window: &mut Window| {
        let Some(first) = points.first() else {
            return;
        };
        let mut path = PathBuilder::fill();
        path.move_to(at(first));
        for p in &points[1..] {
            path.line_to(at(p));
        }
        path.close();
        if let Ok(path) = path.build() {
            window.paint_path(path, rgba(color << 8 | (alpha * 255.).round() as u32));
        }
    };
    fill_with(&shape.body, body, window);
    for eye in &shape.eyes {
        fill_with(eye, zapper_mascots::EYE, window);
    }
}

/// A person: their first initial on a neutral disc.
pub fn person_avatar(name: &str, size: f32) -> Div {
    let initial = name
        .chars()
        .find(|c| c.is_alphanumeric())
        .map(|c| c.to_uppercase().collect::<String>())
        .unwrap_or_else(|| "?".into());
    div()
        .size(px(size))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .bg(rgb(0x3a3a3a))
        .text_color(rgb(0xe8e8e8))
        .text_size(px((size * 0.5).max(7.)))
        .line_height(px(size))
        .font_weight(FontWeight::SEMIBOLD)
        .child(initial)
}

/// The empty assignee slot.
pub fn unassigned_avatar(size: f32) -> Div {
    div()
        .size(px(size))
        .flex_none()
        .rounded_full()
        .border_1()
        .border_dashed()
        .border_color(rgb(0x5a5a5a))
}

#[cfg(test)]
mod tests {
    use super::*;
    // `gpui::*` brings its own `test` attribute; these are plain tests.
    use std::prelude::rust_2024::test;

    fn picture(width: u32, height: u32, noisy: bool) -> Vec<u8> {
        let image = image::RgbaImage::from_fn(width, height, |x, y| {
            if noisy {
                let n = (x.wrapping_mul(2_654_435_761) ^ y.wrapping_mul(40_503)) as u8;
                image::Rgba([n, n.wrapping_mul(7), n.wrapping_mul(13), 255])
            } else if x < width / 2 {
                image::Rgba([255, 0, 0, 255])
            } else {
                image::Rgba([0, 0, 255, 0])
            }
        });
        let mut bytes = Vec::new();
        image
            .write_to(
                &mut std::io::Cursor::new(&mut bytes),
                image::ImageFormat::Png,
            )
            .unwrap();
        bytes
    }

    fn side_of(url: &str) -> (u32, u32) {
        let image = decode_url(url).expect("stored picture decodes");
        let decoded = image::load_from_memory(&image.bytes).unwrap();
        (decoded.width(), decoded.height())
    }

    #[test]
    fn uploads_are_cropped_square_downscaled_and_small() {
        let flat = prepare_image(&picture(900, 300, false)).unwrap();
        assert!(flat.starts_with("data:image/png;base64,"));
        assert_eq!(side_of(&flat), (AVATAR_IMAGE_SIDE, AVATAR_IMAGE_SIDE));

        let photo = prepare_image(&picture(200, 150, true)).unwrap();
        assert!(
            photo.starts_with("data:image/jpeg;base64,"),
            "noisy photos become JPEG"
        );
        assert!(avatar_image_ok(&photo));
        assert_eq!(side_of(&photo), (AVATAR_IMAGE_SIDE, AVATAR_IMAGE_SIDE));

        // Small pictures are cropped but never scaled up.
        let tiny = prepare_image(&picture(40, 64, false)).unwrap();
        assert_eq!(side_of(&tiny), (40, 40));

        assert!(prepare_image(b"not an image").is_err());
    }

    #[test]
    fn faces_prefer_a_readable_picture_and_fall_back_to_the_mascot() {
        let avatar = ZapperAvatar {
            mascot: "hex".into(),
            color: "teal".into(),
        };
        assert!(decoded("data:image/png;base64,???").is_none());
        let url = prepare_image(&picture(64, 64, false)).unwrap();
        assert!(decoded(&url).is_some());

        let mut zapper: Zapper = serde_json::from_str(
            r#"{"id": "z1", "revision": 1,
            "config": {"name": "Ops", "purpose": "", "homeDeviceId": "d", "repositories": [],
                "coordinatorModel": {"harness": "claude-code", "model": null},
                "workerDeviceId": "d", "maxConcurrentWorkers": 4, "maxFollowups": 3,
                "avatar": {"mascot": "hex", "color": "teal"}},
            "status": "active", "coordinatorChatId": "c", "previousCoordinatorChats": [],
            "workers": [], "decisions": []}"#,
        )
        .unwrap();
        assert_eq!(ZapperFace::of(&zapper), ZapperFace::mascot(avatar.clone()));
        zapper.config.avatar_image = Some(url.clone());
        remember_zappers([&zapper]);
        assert_eq!(
            known_face("z1", &avatar).image.as_deref(),
            Some(url.as_str())
        );
        assert_eq!(ZapperFace::of(&zapper).image.as_deref(), Some(url.as_str()));
        zapper.config.avatar_image = None;
        remember_zappers([&zapper]);
        assert_eq!(known_face("z1", &avatar).image, None);
    }

    /// A strip of mascots, one per mood.
    struct Roster(Vec<Mood>);

    impl Render for Roster {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let avatar = ZapperAvatar {
                mascot: "hex".into(),
                color: "teal".into(),
            };
            div().flex().children(
                self.0.iter().map(|mood| {
                    zapper_face(&ZapperFace::mascot(avatar.clone()).with_mood(*mood), 24.)
                }),
            )
        }
    }

    fn live(cx: &mut TestAppContext) -> Vec<Mood> {
        cx.update(|cx| {
            let mut moods: Vec<Mood> = cx
                .default_global::<Motion>()
                .live
                .values()
                .map(|l| l.animator.mood())
                .collect();
            moods.sort_by_key(|m| format!("{m:?}"));
            moods
        })
    }

    #[gpui::test]
    fn mascots_join_the_sampler_when_painted_and_leave_when_gone(cx: &mut TestAppContext) {
        let window = cx.add_window(|_, _| Roster(vec![Mood::Resting, Mood::Working]));
        let handle: AnyWindowHandle = window.into();
        let draw = |cx: &mut TestAppContext| {
            handle
                .update(cx, |_, window, cx| {
                    window.refresh();
                    window.draw(cx).clear();
                })
                .unwrap();
        };
        draw(cx);
        assert_eq!(live(cx), [Mood::Resting, Mood::Working]);
        // A new mood on the same mascot keeps its sampler entry and blends.
        window
            .update(cx, |roster, _, _| {
                roster.0 = vec![Mood::Resting, Mood::Done]
            })
            .unwrap();
        draw(cx);
        assert_eq!(live(cx), [Mood::Done, Mood::Resting]);
        // The resting mascot never moves, so it never asks for a repaint; it
        // still leaves once its view paints without it.
        window
            .update(cx, |roster, _, _| roster.0 = vec![Mood::Done])
            .unwrap();
        std::thread::sleep(SAME_FRAME * 2);
        draw(cx);
        cx.update(sample);
        assert_eq!(live(cx).len(), 1);
    }
}
