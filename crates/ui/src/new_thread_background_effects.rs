//! Effects remain cached source-space images; a separate alpha mask follows layout.
use crate::settings::{NewThreadBackgroundAdjustment, NewThreadBackgroundEffect};
use crate::theme::Theme;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

/// Hold the displayed artwork during loading, then crossfade to the ready image.
/// Finish each blend before adopting another image to keep rapid changes smooth.
#[derive(Default)]
pub(crate) struct Readiness {
    current: Option<Arc<gpui::RenderImage>>,
    previous: Option<Arc<gpui::RenderImage>>,
    current_adjustment: NewThreadBackgroundAdjustment,
    previous_adjustment: NewThreadBackgroundAdjustment,
    current_path: Option<PathBuf>,
    previous_path: Option<PathBuf>,
    started: Option<Instant>,
}

pub(crate) struct ArtworkFrame {
    pub current: Option<Arc<gpui::RenderImage>>,
    pub previous: Option<Arc<gpui::RenderImage>>,
    pub current_adjustment: NewThreadBackgroundAdjustment,
    pub previous_adjustment: NewThreadBackgroundAdjustment,
    pub mix: f32,
    pub active: bool,
}

fn crossfade_mix(start: Instant, now: Instant) -> f32 {
    let spec = crate::motion::WALLPAPER_CROSSFADE;
    let duration = spec.total().mul_f32(crate::motion::speed_scale());
    spec.progress(now.saturating_duration_since(start).as_secs_f32() / duration.as_secs_f32())
}

impl Readiness {
    pub fn frame(
        &mut self,
        image: Option<Arc<gpui::RenderImage>>,
        path: Option<&Path>,
        adjustment: NewThreadBackgroundAdjustment,
        enabled: bool,
        reduced: bool,
        now: Instant,
    ) -> ArtworkFrame {
        let progress = self.started.map_or(1.0, |start| crossfade_mix(start, now));
        if reduced || progress >= 1.0 {
            self.previous = None;
            self.previous_path = None;
            self.started = None;
        }
        // Effects may still be loading, but a crop belongs to the source path,
        // not its effect raster. Replacement images keep their own framing.
        if path.is_some() && path == self.current_path.as_deref() {
            self.current_adjustment = adjustment.normalized();
        }
        if path.is_some() && path == self.previous_path.as_deref() {
            self.previous_adjustment = adjustment.normalized();
        }
        // None while enabled means "still loading", not "remove artwork".
        if self.started.is_none() && (!enabled || image.is_some()) {
            let target = if enabled { image } else { None };
            if self.current.as_ref().map(|image| image.id) != target.as_ref().map(|image| image.id)
            {
                self.previous = self.current.take();
                self.previous_adjustment = self.current_adjustment;
                self.previous_path = self.current_path.take();
                self.current = target;
                self.current_adjustment = adjustment.normalized();
                self.current_path = path.map(Path::to_path_buf);
                if reduced {
                    self.previous = None;
                    self.previous_path = None;
                } else {
                    self.started = Some(now);
                }
            }
        }
        ArtworkFrame {
            current: self.current.clone(),
            previous: self.previous.clone(),
            current_adjustment: self.current_adjustment,
            previous_adjustment: self.previous_adjustment,
            mix: self.started.map_or(1.0, |start| crossfade_mix(start, now)),
            active: self.started.is_some(),
        }
    }
}
type EffectEntry = (
    (NewThreadBackgroundEffect, bool),
    Option<Arc<gpui::RenderImage>>,
);
type PhotoKey = (gpui::ImageId, u8, u8);

#[derive(Debug, Default)]
struct PhotoRaster {
    pending: bool,
    ready: Option<(PhotoKey, Arc<gpui::RenderImage>)>,
}

/// One decoded frame at raster resolution: color and luma per pixel.
#[derive(Debug)]
struct FrameData {
    width: u32,
    height: u32,
    pixels: Box<[u8]>,
    colors: Box<[[u8; 4]]>,
    /// Mean luma and alpha per cell of a coarse `GRID`×`GRID` grid, so the
    /// heading's backdrop can be measured for any crop without a pixel scan.
    grid: Box<[[u8; 2]]>,
}

const GRID: u32 = 32;

impl FrameData {
    fn new(width: u32, height: u32, pixels: Box<[u8]>, colors: Box<[[u8; 4]]>) -> Self {
        let mut sums = vec![[0u64; 3]; (GRID * GRID) as usize];
        for y in 0..height {
            for x in 0..width {
                let cell = (y * GRID / height.max(1) * GRID + x * GRID / width.max(1)) as usize;
                let index = (y * width + x) as usize;
                sums[cell][0] += u64::from(pixels[index]);
                sums[cell][1] += u64::from(colors[index][3]);
                sums[cell][2] += 1;
            }
        }
        let grid = sums
            .iter()
            .map(|[luma, alpha, count]| {
                let count = (*count).max(1);
                [(luma / count) as u8, (alpha / count) as u8]
            })
            .collect();
        Self {
            width,
            height,
            pixels,
            colors,
            grid,
        }
    }

    /// The luma (0–255) seen across the normalized `[x0, y0, x1, y1]`
    /// region, each cell composited over `canvas` by its own alpha and the
    /// image brightened by `gain`.
    fn region_luma(&self, [x0, y0, x1, y1]: [f32; 4], canvas: f32, gain: f32) -> f32 {
        let span = |from: f32, to: f32| {
            let first = ((from * GRID as f32).floor().max(0.0) as u32).min(GRID - 1);
            let last = ((to * GRID as f32).ceil() as u32).clamp(first + 1, GRID);
            first..last
        };
        let (mut sum, mut count) = (0.0_f32, 0.0_f32);
        for row in span(y0, y1) {
            for column in span(x0, x1) {
                let [luma, alpha] = self.grid[(row * GRID + column) as usize];
                let alpha = f32::from(alpha) / 255.0;
                sum += (f32::from(luma) * gain).min(255.0) * alpha + canvas * (1.0 - alpha);
                count += 1.0;
            }
        }
        sum / count.max(1.0)
    }
}

#[derive(Debug)]
struct BackgroundLuminance {
    /// Every frame with its display time; a still image has exactly one.
    frames: Vec<(FrameData, image::Delay)>,
    effects: Mutex<Vec<EffectEntry>>,
    photo: Mutex<PhotoRaster>,
}

/// Effects and color sampling read the first frame unless they iterate.
impl std::ops::Deref for BackgroundLuminance {
    type Target = FrameData;

    fn deref(&self) -> &FrameData {
        &self.frames[0].0
    }
}
impl BackgroundLuminance {
    fn raster_image(
        self: &Arc<Self>,
        effect: NewThreadBackgroundEffect,
        light: bool,
        cx: &mut gpui::App,
    ) -> Option<Arc<gpui::RenderImage>> {
        let mut effects = self.effects.lock().unwrap();
        let key = (
            effect,
            light
                && !matches!(
                    effect,
                    NewThreadBackgroundEffect::Dither
                        | NewThreadBackgroundEffect::None
                        | NewThreadBackgroundEffect::Grain
                ),
        );
        if let Some((_, image)) = effects.iter().find(|(cached, _)| *cached == key) {
            return image.clone();
        }
        // None marks the single pending job for this source/effect, not a viewport.
        effects.push((key, None));
        drop(effects);
        let source = self.clone();
        cx.spawn(async move |cx| {
            let worker = source.clone();
            let image = cx
                .background_executor()
                .spawn(async move { worker.render_image(effect, light) })
                .await;
            cx.update(|cx| {
                let evicted = {
                    let mut effects = source.effects.lock().unwrap();
                    if let Some((_, ready)) = effects.iter_mut().find(|(cached, _)| *cached == key)
                    {
                        *ready = Some(image);
                    }
                    // An animation's raster holds every frame, so keep only
                    // a few. Evict the oldest finished ones beyond that cap,
                    // never the one just made: views showing different
                    // effects at once (hero and Compare original) both keep
                    // theirs instead of evicting each other in a loop.
                    let mut evicted = Vec::new();
                    if source.frames.len() > 1 {
                        let ready = effects.iter().filter(|(_, image)| image.is_some()).count();
                        let mut excess = ready.saturating_sub(MAX_ANIMATED_RASTERS);
                        effects.retain_mut(|(cached, image)| {
                            if excess > 0 && *cached != key && image.is_some() {
                                excess -= 1;
                                evicted.extend(image.take());
                                false
                            } else {
                                true
                            }
                        });
                    }
                    evicted
                };
                for image in evicted {
                    cx.drop_image(image, None);
                }
                cx.refresh_windows();
            });
        })
        .detach();
        None
    }
    /// The effect raster, or the plain image while the effect is still being
    /// computed, so switching effects never blanks the backdrop.
    fn effect_or_plain(
        self: &Arc<Self>,
        effect: NewThreadBackgroundEffect,
        light: bool,
        cx: &mut gpui::App,
    ) -> Option<Arc<gpui::RenderImage>> {
        self.raster_image(effect, light, cx)
            .or_else(|| self.raster_image(NewThreadBackgroundEffect::None, light, cx))
    }

    fn render_image(
        &self,
        effect: NewThreadBackgroundEffect,
        light: bool,
    ) -> Arc<gpui::RenderImage> {
        let frames = self
            .frames
            .iter()
            .map(|(frame, delay)| {
                image::Frame::from_parts(frame.effect_pixels(effect, light), 0, 0, *delay)
            })
            .collect::<Vec<_>>();
        Arc::new(gpui::RenderImage::new(frames))
    }
}

impl FrameData {
    /// This frame through `effect`, as the BGRA bytes `RenderImage` consumes.
    fn effect_pixels(&self, effect: NewThreadBackgroundEffect, light: bool) -> image::RgbaImage {
        match effect {
            NewThreadBackgroundEffect::None => {
                image::RgbaImage::from_fn(self.width, self.height, |x, y| {
                    let [r, g, b, a] = self.rgba(x, y);
                    image::Rgba([b, g, r, a])
                })
            }
            NewThreadBackgroundEffect::Dither => self.dither_pixels(),
            NewThreadBackgroundEffect::Grain => self.grain_pixels(),
            NewThreadBackgroundEffect::Vignette => self.vignette_pixels(light),
            NewThreadBackgroundEffect::Scanlines => self.scanline_pixels(light),
        }
    }

    /// Source pixels per effect "dot". Rasters are drawn at source resolution
    /// (up to 2048px) and scaled to the window, so a fixed 1–2px pattern
    /// vanishes or turns to moiré on screen; scale it with the image instead.
    fn unit(&self) -> u32 {
        (self.width.max(self.height) / 680).max(1)
    }

    fn rgba(&self, x: u32, y: u32) -> [u8; 4] {
        self.colors[(y.min(self.height - 1) * self.width + x.min(self.width - 1)) as usize]
    }

    fn luma(&self, x: u32, y: u32) -> u8 {
        self.pixels[(y.min(self.height - 1) * self.width + x.min(self.width - 1)) as usize]
    }

    /// Film grain: clustered monochrome noise, strongest in the midtones
    /// where real grain reads, gentle in deep shadows and highlights.
    fn grain_pixels(&self) -> image::RgbaImage {
        let size = (self.unit() / 2).max(1);
        image::RgbaImage::from_fn(self.width, self.height, |x, y| {
            let [red, green, blue, alpha] = self.rgba(x, y);
            let noise = hash_unit(x / size, y / size) * 2.0 - 1.0;
            let tone = f32::from(self.luma(x, y)) / 255.0;
            let amount = 34.0 * (0.35 + 0.65 * (1.0 - (2.0 * tone - 1.0).abs()));
            let grain = |channel: u8| (f32::from(channel) + noise * amount).clamp(0.0, 255.0) as u8;
            image::Rgba([grain(blue), grain(green), grain(red), alpha])
        })
    }

    fn vignette_pixels(&self, light: bool) -> image::RgbaImage {
        let width = self.width.saturating_sub(1).max(1) as f32;
        let height = self.height.saturating_sub(1).max(1) as f32;
        image::RgbaImage::from_fn(self.width, self.height, |x, y| {
            let [red, green, blue, alpha] = self.colors[(y * self.width + x) as usize];
            let horizontal = 2.0 * x as f32 / width - 1.0;
            let vertical = 2.0 * y as f32 / height - 1.0;
            let distance = (horizontal * horizontal + vertical * vertical) * 0.5;
            let edge = ((distance - 0.15) / 0.85).clamp(0.0, 1.0);
            let strength = edge * edge * (3.0 - 2.0 * edge) * if light { 0.30 } else { 0.55 };
            let paper = if light { 255.0 } else { 0.0 };
            let fade =
                |channel: u8| (f32::from(channel) * (1.0 - strength) + paper * strength) as u8;
            image::Rgba([fade(blue), fade(green), fade(red), alpha])
        })
    }

    /// CRT scanlines: a dark line every few screen pixels, with a slight
    /// bloom on the lit rows so the image keeps its brightness.
    fn scanline_pixels(&self, light: bool) -> image::RgbaImage {
        let line = self.unit().max(2);
        let period = line * 3;
        image::RgbaImage::from_fn(self.width, self.height, |x, y| {
            let [r, g, b, a] = self.rgba(x, y);
            let dark = y % period < line;
            let channel = |value: u8| {
                let value = f32::from(value);
                (if light {
                    if dark {
                        value + (255.0 - value) * 0.42
                    } else {
                        value
                    }
                } else if dark {
                    value * 0.38
                } else {
                    value * 1.08
                })
                .clamp(0.0, 255.0) as u8
            };
            image::Rgba([channel(b), channel(g), channel(r), a])
        })
    }

    /// Ordered (Bayer) dither: each chunky dot is either its color at full
    /// strength or near-black, so gradients turn into a dot pattern.
    fn dither_pixels(&self) -> image::RgbaImage {
        const BAYER: [[u8; 4]; 4] = [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]];
        let dot = self.unit().max(2);
        image::RgbaImage::from_fn(self.width, self.height, |x, y| {
            let (dx, dy) = (x / dot, y / dot);
            let [r, g, b, _] = dither_color(
                self.rgba(dx * dot + dot / 2, dy * dot + dot / 2),
                BAYER[dy as usize % 4][dx as usize % 4],
            );
            // RenderImage consumes BGRA.
            image::Rgba([b, g, r, self.rgba(x, y)[3]])
        })
    }
}

type Source = Arc<Mutex<Option<Arc<BackgroundLuminance>>>>;
type Cache = Vec<(PathBuf, Source)>;
static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();

fn decode_source(bytes: &[u8]) -> Option<Arc<BackgroundLuminance>> {
    let image = crate::new_thread_background_image::decode(bytes).ok()?;
    Some(source_from_bytes(bytes, &image))
}

/// Animated GIF/WebP sources keep their frames; anything else is a still.
fn source_from_bytes(bytes: &[u8], first: &image::DynamicImage) -> Arc<BackgroundLuminance> {
    match animation_frames(bytes) {
        Some(frames) => source_from_frames(frames),
        None => source_from_image(first),
    }
}

fn source_from_image(image: &image::DynamicImage) -> Arc<BackgroundLuminance> {
    source_from_frames(vec![(
        frame_data(image, STILL_MAX_SIDE),
        image::Delay::from_numer_denom_ms(0, 1),
    )])
}

fn source_from_frames(frames: Vec<(FrameData, image::Delay)>) -> Arc<BackgroundLuminance> {
    Arc::new(BackgroundLuminance {
        frames,
        effects: Mutex::new(Vec::new()),
        photo: Mutex::default(),
    })
}

fn frame_data(image: &image::DynamicImage, max_side: u32) -> FrameData {
    let proxy = image.thumbnail(max_side, max_side);
    let gray = proxy.to_luma8();
    let (width, height) = gray.dimensions();
    FrameData::new(
        width,
        height,
        gray.into_raw().into_boxed_slice(),
        proxy.to_rgba8().pixels().map(|pixel| pixel.0).collect(),
    )
}

const STILL_MAX_SIDE: u32 = 2048;
/// Finished effect rasters kept per animated source.
const MAX_ANIMATED_RASTERS: usize = 3;
/// Animated frames are smaller: every frame is held decoded, and again per
/// effect raster, so the whole animation shares a pixel budget.
const ANIMATION_MAX_SIDE: u32 = 960;
const ANIMATION_PIXEL_BUDGET: u64 = 24_000_000;
const ANIMATION_MAX_FRAMES: usize = 240;

/// Decode an animated GIF or WebP frame by frame, shrinking each as it
/// arrives so a long, large animation never sits in memory at full size.
/// Stops at the frame or pixel budget; returns `None` for stills.
fn animation_frames(bytes: &[u8]) -> Option<Vec<(FrameData, image::Delay)>> {
    use image::AnimationDecoder as _;
    let cursor = std::io::Cursor::new(bytes);
    let frames = match image::guess_format(bytes).ok()? {
        image::ImageFormat::Gif => image::codecs::gif::GifDecoder::new(cursor)
            .ok()?
            .into_frames(),
        image::ImageFormat::WebP => {
            let decoder = image::codecs::webp::WebPDecoder::new(cursor).ok()?;
            if !decoder.has_animation() {
                return None;
            }
            decoder.into_frames()
        }
        _ => return None,
    };
    let mut decoded = Vec::new();
    let mut budget = ANIMATION_PIXEL_BUDGET;
    for frame in frames.take(ANIMATION_MAX_FRAMES) {
        let frame = frame.ok()?;
        let delay = playable_delay(frame.delay());
        let data = frame_data(
            &image::DynamicImage::ImageRgba8(frame.into_buffer()),
            ANIMATION_MAX_SIDE,
        );
        let pixels = u64::from(data.width) * u64::from(data.height);
        if pixels > budget {
            break;
        }
        budget -= pixels;
        decoded.push((data, delay));
    }
    (decoded.len() > 1).then_some(decoded)
}

/// Browsers treat GIF delays under 20ms as 100ms; so do we, so "as fast as
/// possible" animations don't spin the renderer.
fn playable_delay(delay: image::Delay) -> image::Delay {
    let (numer, denom) = delay.numer_denom_ms();
    if numer < denom.saturating_mul(20) {
        image::Delay::from_numer_denom_ms(100, 1)
    } else {
        delay
    }
}

/// Fully decoded source and selected effect, built on the background executor.
pub(crate) struct PreloadedArtwork(Arc<BackgroundLuminance>);

impl PreloadedArtwork {
    /// Takes the image the caller already decoded to validate the candidate,
    /// so a full-resolution wallpaper is decoded once per preload.
    pub fn load(
        bytes: &[u8],
        image: &image::DynamicImage,
        effect: NewThreadBackgroundEffect,
        light: bool,
    ) -> Self {
        let source = source_from_bytes(bytes, image);
        let light = light
            && !matches!(
                effect,
                NewThreadBackgroundEffect::None
                    | NewThreadBackgroundEffect::Dither
                    | NewThreadBackgroundEffect::Grain
            );
        let image = source.render_image(effect, light);
        source
            .effects
            .lock()
            .unwrap()
            .push(((effect, light), Some(image)));
        Self(source)
    }

    pub fn color(&self) -> Option<zuse_theme::Color> {
        let stride = (self.0.colors.len() / 4096).max(1);
        crate::settings::wallpaper_colors::extract(self.0.colors.iter().step_by(stride).copied())
    }

    pub fn install(self, path: &Path) {
        let mut cache = CACHE.get_or_init(|| Mutex::new(Vec::new())).lock().unwrap();
        cache.retain(|(key, _)| key != path);
        cache.push((path.to_path_buf(), Arc::new(Mutex::new(Some(self.0)))));
        if cache.len() > 4 {
            cache.remove(0);
        }
    }
}

/// Whether the wallpaper at `path` has decoded into drawable artwork. A file
/// that exists but cannot be read never becomes ready.
pub(crate) fn artwork_ready(path: &Path) -> bool {
    CACHE.get().is_some_and(|cache| {
        cache.lock().is_ok_and(|cache| {
            cache.iter().any(|(key, source)| {
                key == path && source.lock().is_ok_and(|source| source.is_some())
            })
        })
    })
}

fn background_luminance(path: &Path, cx: &mut gpui::App) -> Option<Arc<BackgroundLuminance>> {
    let cache = CACHE.get_or_init(|| Mutex::new(Vec::new()));
    if let Some(source) = cache
        .lock()
        .ok()?
        .iter()
        .find_map(|(key, source)| (key == path).then(|| source.clone()))
    {
        return source.lock().ok()?.clone();
    }
    let pending = Arc::new(Mutex::new(None));
    {
        let mut cache = cache.lock().ok()?;
        cache.push((path.to_path_buf(), pending.clone()));
        if cache.len() > 4 {
            cache.remove(0);
        }
    }
    let path = path.to_path_buf();
    cx.spawn(async move |cx| {
        let source = cx
            .background_executor()
            .spawn(async move {
                let bytes = std::fs::read(path).ok()?;
                decode_source(&bytes)
            })
            .await;
        cx.update(|cx| {
            *pending.lock().unwrap() = source;
            cx.refresh_windows();
        });
    })
    .detach();
    None
}

/// Whether the wallpaper behind the new-thread heading reads as dark, as it
/// is drawn: the crop `adjustment` frames for a `hero`-sized area, the
/// frame on screen now (the first under reduced motion), each pixel's alpha
/// and the brightness setting, then the whole blended over `canvas` at
/// `opacity`. `None` until the image decodes.
#[allow(clippy::too_many_arguments)]
pub(crate) fn backdrop_is_dark(
    path: &Path,
    adjustment: NewThreadBackgroundAdjustment,
    hero: (f32, f32),
    look: crate::settings::WallpaperLook,
    canvas: gpui::Hsla,
    opacity: f32,
    animate: bool,
    cx: &mut gpui::App,
) -> Option<bool> {
    use crate::new_thread_background_mask::{clock_ms, crop_for_adjustment, frame_at};
    let source = background_luminance(path, cx)?;
    let frame = if animate && source.frames.len() > 1 {
        let delays = source.frames.iter().map(|(_, delay)| {
            let (numer, denom) = delay.numer_denom_ms();
            u64::from(numer) / u64::from(denom.max(1))
        });
        frame_at(delays, clock_ms())
    } else {
        0
    };
    let frame = &source.frames[frame].0;
    let [x, y, width, height] =
        crop_for_adjustment((frame.width as f32, frame.height as f32), hero, adjustment);
    // The heading sits in the hero's upper-middle band.
    let band = [x, y + height * 0.2, x + width, y + height * 0.6];
    let canvas = canvas.l * 255.0;
    let image = frame.region_luma(band, canvas, f32::from(look.brightness) / 100.0);
    let alpha = (opacity * f32::from(look.opacity) / 100.0).clamp(0.0, 1.0);
    Some(image * alpha + canvas * (1.0 - alpha) < 140.0)
}

/// Safe to call on both routes: loading/decoding/effects happen once off-thread,
/// before the hero is requested, and never depend on composer/sidebar geometry.
pub(super) fn prepare(
    effect: NewThreadBackgroundEffect,
    theme: &Theme,
    path: &Path,
    cx: &mut gpui::App,
) -> Option<Arc<gpui::RenderImage>> {
    let light = matches!(theme.appearance, crate::theme::Appearance::Light);
    let source = background_luminance(path, cx)?;
    source.effect_or_plain(effect, light, cx)
}

pub(crate) fn prepare_look(
    effect: NewThreadBackgroundEffect,
    theme: &Theme,
    path: &Path,
    look: crate::settings::WallpaperLook,
    cx: &mut gpui::App,
) -> Option<Arc<gpui::RenderImage>> {
    if look.brightness == 100 && look.blur == 0 {
        return prepare(effect, theme, path, cx);
    }
    let source = background_luminance(path, cx)?;
    let base = source.effect_or_plain(effect, theme.appearance.is_light(), cx)?;
    let key = (base.id, look.brightness, look.blur);
    let mut cache = source.photo.lock().unwrap();
    let ready = cache.ready.as_ref().filter(|(cached, _)| cached.0 == key.0);
    if let Some((_, image)) = ready.filter(|(cached, _)| *cached == key) {
        return Some(image.clone());
    }
    let fallback = ready.map_or_else(|| base.clone(), |(_, image)| image.clone());
    if !cache.pending {
        cache.pending = true;
        drop(cache);
        cx.spawn(async move |cx| {
            let image = cx
                .background_executor()
                .spawn(async move { photo_raster(base, look.brightness, look.blur) })
                .await;
            cx.update(|cx| {
                let replaced = {
                    let mut cache = source.photo.lock().unwrap();
                    cache.pending = false;
                    cache.ready.replace((key, image))
                };
                // Each slider step makes a full-size raster; release the last
                // one's GPU texture instead of letting the atlas grow.
                if let Some((_, old)) = replaced {
                    cx.drop_image(old, None);
                }
                cx.refresh_windows();
            });
        })
        .detach();
    }
    Some(fallback)
}

fn photo_raster(base: Arc<gpui::RenderImage>, brightness: u8, blur: u8) -> Arc<gpui::RenderImage> {
    let frames = (0..base.frame_count())
        .map(|index| {
            let size = base.size(index);
            let pixels = base.as_bytes(index).and_then(|bytes| {
                image::RgbaImage::from_raw(
                    size.width.0 as u32,
                    size.height.0 as u32,
                    bytes.to_vec(),
                )
            })?;
            Some(image::Frame::from_parts(
                adjust_photo(pixels, brightness, blur),
                0,
                0,
                base.delay(index),
            ))
        })
        .collect::<Option<Vec<_>>>();
    match frames {
        Some(frames) => Arc::new(gpui::RenderImage::new(frames)),
        None => base,
    }
}

fn adjust_photo(mut pixels: image::RgbaImage, brightness: u8, blur: u8) -> image::RgbaImage {
    if brightness != 100 {
        let gain = f32::from(brightness) / 100.0;
        for pixel in pixels.pixels_mut() {
            for channel in &mut pixel.0[..3] {
                *channel = (f32::from(*channel) * gain).round().clamp(0.0, 255.0) as u8;
            }
        }
    }
    if blur > 0 {
        // A wide Gaussian at full resolution takes seconds and makes the
        // sliders lag. Blur has no fine detail to keep: shrink until the radius
        // is a few pixels, blur there, and let the GPU scale it back up.
        let sigma = f32::from(blur) * pixels.height() as f32 / 650.0;
        let shrink = (sigma / 3.0).max(1.0);
        if shrink > 1.0 {
            pixels = image::imageops::resize(
                &pixels,
                ((pixels.width() as f32 / shrink).round() as u32).max(1),
                ((pixels.height() as f32 / shrink).round() as u32).max(1),
                image::imageops::FilterType::Triangle,
            );
        }
        pixels = image::imageops::fast_blur(&pixels, sigma / shrink);
    }
    pixels
}
/// A stable pseudo-random value in [0, 1) per cell.
fn hash_unit(x: u32, y: u32) -> f32 {
    let mut seed = x.wrapping_mul(0x9e37_79b1) ^ y.wrapping_mul(0x85eb_ca77);
    seed = (seed ^ (seed >> 16)).wrapping_mul(0x7feb_352d);
    seed = (seed ^ (seed >> 15)).wrapping_mul(0x846c_a68b);
    (seed ^ (seed >> 16)) as f32 / u32::MAX as f32
}

fn dither_color([r, g, b, a]: [u8; 4], threshold: u8) -> [u8; 4] {
    let peak = r.max(g).max(b) as f32;
    let bright = peak / 255.0 > (threshold as f32 + 0.5) / 16.0;
    let gain = if bright { 255.0 / peak.max(1.0) } else { 0.08 };
    [
        (r as f32 * gain).round() as u8,
        (g as f32 * gain).round() as u8,
        (b as f32 * gain).round() as u8,
        a,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui::test]
    fn animated_gifs_keep_every_frame_through_effects(cx: &mut gpui::TestAppContext) {
        let mut bytes = Vec::new();
        {
            let mut encoder = image::codecs::gif::GifEncoder::new(&mut bytes);
            for shade in [40u8, 140, 240] {
                encoder
                    .encode_frame(image::Frame::from_parts(
                        image::RgbaImage::from_pixel(32, 24, image::Rgba([shade, 20, 20, 255])),
                        0,
                        0,
                        // Zero delay: played at the browsers' 100ms instead.
                        image::Delay::from_numer_denom_ms(0, 1),
                    ))
                    .unwrap();
            }
        }
        let source = decode_source(&bytes).unwrap();
        assert_eq!(source.frames.len(), 3);
        assert!(
            source
                .frames
                .iter()
                .all(|(_, delay)| delay.numer_denom_ms() == (100, 1))
        );
        cx.update(|cx| source.raster_image(NewThreadBackgroundEffect::Grain, false, cx));
        cx.run_until_parked();
        cx.update(|cx| {
            let raster = source
                .raster_image(NewThreadBackgroundEffect::Grain, false, cx)
                .unwrap();
            assert_eq!(raster.frame_count(), 3);
        });
        // A still image stays a single frame.
        let mut png = Vec::new();
        image::RgbaImage::from_pixel(8, 8, image::Rgba([9, 9, 9, 255]))
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        assert_eq!(decode_source(&png).unwrap().frames.len(), 1);
    }

    #[test]
    fn heading_band_tracks_wallpaper_darkness() {
        let flat = |value| {
            image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
                64,
                48,
                image::Rgba([value, value, value, 255]),
            ))
        };
        let band = [0.0, 0.2, 1.0, 0.6];
        assert!(source_from_image(&flat(20)).region_luma(band, 255.0, 1.0) < 140.0);
        assert!(source_from_image(&flat(230)).region_luma(band, 0.0, 1.0) >= 140.0);
        // Transparent black over a light canvas reads light, not dark.
        let clear = image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            64,
            48,
            image::Rgba([0, 0, 0, 0]),
        ));
        assert!(source_from_image(&clear).region_luma(band, 240.0, 1.0) > 200.0);
        // Only the cropped region counts: black top half, white bottom half.
        let split = image::DynamicImage::ImageRgba8(image::RgbaImage::from_fn(64, 64, |_, y| {
            let value = if y < 32 { 0 } else { 255 };
            image::Rgba([value, value, value, 255])
        }));
        let split = source_from_image(&split);
        assert!(split.region_luma([0.0, 0.6, 1.0, 0.9], 0.0, 1.0) > 200.0);
        assert!(split.region_luma([0.0, 0.1, 1.0, 0.4], 255.0, 1.0) < 50.0);
    }

    fn artwork() -> Arc<gpui::RenderImage> {
        Arc::new(gpui::RenderImage::new([image::Frame::new(
            image::RgbaImage::new(1, 1),
        )]))
    }

    #[test]
    fn wallpaper_crossfade_holds_while_loading_and_handles_rapid_changes() {
        let first = artwork();
        let next = artwork();
        let latest = artwork();
        let mut ready = Readiness::default();
        let now = Instant::now();
        let path = Some(Path::new("background.png"));
        let adjustment = NewThreadBackgroundAdjustment::default();
        ready.frame(Some(first.clone()), path, adjustment, true, true, now);
        let loading = ready.frame(None, path, adjustment, true, false, now);
        assert_eq!(loading.current.unwrap().id, first.id);
        assert!(!loading.active);
        let start = ready.frame(Some(next.clone()), path, adjustment, true, false, now);
        assert_eq!(start.mix, 0.0);
        assert_eq!(start.previous.unwrap().id, first.id);
        let halfway = now
            + std::time::Duration::from_secs_f32(
                crate::motion::WALLPAPER_CROSSFADE.total().as_secs_f32()
                    * crate::motion::speed_scale()
                    * 0.5,
            );
        let frame = ready.frame(Some(latest.clone()), path, adjustment, true, false, halfway);
        assert!((frame.mix - 0.875).abs() < 0.001);
        assert_eq!(frame.current.unwrap().id, next.id);
        let done = now + std::time::Duration::from_secs(1);
        let frame = ready.frame(Some(latest.clone()), path, adjustment, true, false, done);
        assert_eq!(frame.previous.unwrap().id, next.id);
        assert_eq!(frame.current.unwrap().id, latest.id);
        assert_eq!(frame.mix, 0.0);
        let frame = ready.frame(Some(latest), path, adjustment, true, true, done);
        assert!(!frame.active);
        assert!(frame.previous.is_none());
        assert_eq!(frame.mix, 1.0);
        let removed = ready.frame(None, None, adjustment, false, true, done);
        assert!(removed.current.is_none() && removed.previous.is_none());
    }

    #[test]
    fn wallpaper_replacement_and_removal_keep_the_departing_crop() {
        let first = artwork();
        let next = artwork();
        let first_path = Some(Path::new("first.png"));
        let next_path = Some(Path::new("next.png"));
        let cropped = NewThreadBackgroundAdjustment {
            focal_x: 0.2,
            focal_y: 0.8,
            zoom: 2.0,
        };
        let centered = NewThreadBackgroundAdjustment::default();
        let mut ready = Readiness::default();
        let now = Instant::now();
        ready.frame(Some(first.clone()), first_path, cropped, true, true, now);
        let loading = ready.frame(None, next_path, centered, true, false, now);
        assert_eq!(loading.current.as_ref().unwrap().id, first.id);
        assert_eq!(loading.current_adjustment, cropped);
        let start = ready.frame(Some(next), next_path, centered, true, false, now);
        assert_eq!(start.previous_adjustment, cropped);
        assert_eq!(start.current_adjustment, centered);
        let latest = artwork();
        let latest_path = Some(Path::new("latest.png"));
        let waiting = ready.frame(Some(latest.clone()), latest_path, cropped, true, false, now);
        assert_eq!(waiting.previous_adjustment, cropped);
        assert_eq!(waiting.current_adjustment, centered);
        let done = now + std::time::Duration::from_secs(1);
        let adopted = ready.frame(Some(latest), latest_path, cropped, true, false, done);
        assert_eq!(adopted.previous_adjustment, centered);
        assert_eq!(adopted.current_adjustment, cropped);

        let mut ready = Readiness::default();
        ready.frame(Some(first), first_path, cropped, true, true, now);
        let removed = ready.frame(None, None, centered, false, false, now);
        assert!(removed.current.is_none());
        assert!(removed.previous.is_some());
        assert_eq!(removed.previous_adjustment, cropped);
    }

    #[test]
    fn applying_crop_during_effect_loading_updates_the_same_source_without_a_fade() {
        let first = artwork();
        let effect = artwork();
        let path = Some(Path::new("background.png"));
        let centered = NewThreadBackgroundAdjustment::default();
        let cropped = NewThreadBackgroundAdjustment {
            zoom: 2.0,
            ..centered
        };
        let latest = NewThreadBackgroundAdjustment {
            zoom: 3.0,
            ..centered
        };
        let mut ready = Readiness::default();
        let now = Instant::now();
        ready.frame(Some(first), path, centered, true, true, now);
        let loading = ready.frame(None, path, cropped, true, false, now);
        assert_eq!(loading.current_adjustment, cropped);
        assert!(!loading.active);
        let start = ready.frame(Some(effect.clone()), path, cropped, true, false, now);
        assert_eq!(start.previous_adjustment, cropped);
        assert_eq!(start.current_adjustment, cropped);
        let applied = ready.frame(Some(effect.clone()), path, latest, true, false, now);
        assert_eq!(applied.previous_adjustment, latest);
        assert_eq!(applied.current_adjustment, latest);
        let done = now + std::time::Duration::from_secs(1);
        let applied = ready.frame(Some(effect), path, cropped, true, false, done);
        assert_eq!(applied.current_adjustment, cropped);
        assert!(!applied.active);
    }

    #[gpui::test]
    fn prewarming_decodes_off_thread_and_reuses_artwork_without_hero_geometry(
        cx: &mut gpui::TestAppContext,
    ) {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("background.png");
        image::RgbaImage::from_pixel(32, 24, image::Rgba([173, 89, 231, 180]))
            .save(&path)
            .unwrap();
        cx.update(|cx| assert!(background_luminance(&path, cx).is_none()));
        cx.run_until_parked();
        let source = cx.update(|cx| background_luminance(&path, cx).unwrap());
        cx.update(|cx| {
            assert!(
                source
                    .raster_image(NewThreadBackgroundEffect::None, false, cx)
                    .is_none()
            )
        });
        cx.run_until_parked();
        cx.update(|cx| {
            let warm = background_luminance(&path, cx).unwrap();
            assert!(Arc::ptr_eq(&source, &warm));
            let image = warm
                .raster_image(NewThreadBackgroundEffect::None, false, cx)
                .unwrap();
            assert_eq!(image.as_bytes(0).unwrap()[..4], [231, 89, 173, 180]);
            for _ in 0..20 {
                assert!(Arc::ptr_eq(
                    &image,
                    &warm
                        .raster_image(NewThreadBackgroundEffect::None, false, cx)
                        .unwrap()
                ));
            }
        });
    }
    fn fixture() -> Arc<BackgroundLuminance> {
        source_from_frames(vec![(
            FrameData::new(
                60,
                32,
                vec![128; 1920].into_boxed_slice(),
                vec![[128, 64, 32, 200]; 1920].into_boxed_slice(),
            ),
            image::Delay::from_numer_denom_ms(0, 1),
        )])
    }
    #[gpui::test]
    fn every_effect_is_generated_once_independently_of_viewport(cx: &mut gpui::TestAppContext) {
        let source = fixture();
        for effect in NewThreadBackgroundEffect::ALL {
            cx.update(|cx| {
                for _ in 0..100 {
                    assert!(source.raster_image(effect, false, cx).is_none());
                }
                assert_eq!(
                    source
                        .effects
                        .lock()
                        .unwrap()
                        .iter()
                        .filter(|(key, _)| *key == (effect, false))
                        .count(),
                    1
                );
            });
            cx.run_until_parked();
            cx.update(|cx| {
                let first = source.raster_image(effect, false, cx).unwrap();
                assert_eq!(first.size(0).width.0, 60);
                assert_eq!(first.size(0).height.0, 32);
                for _ in 0..100 {
                    assert!(Arc::ptr_eq(
                        &first,
                        &source.raster_image(effect, false, cx).unwrap()
                    ));
                }
            });
        }
    }
    #[test]
    fn light_treatments_use_light_paper_without_inverting_source_hues() {
        let source = fixture();
        for (light, dark) in [
            (source.scanline_pixels(true), source.scanline_pixels(false)),
            (source.vignette_pixels(true), source.vignette_pixels(false)),
        ] {
            let brightness = |image: &image::RgbaImage| -> u64 {
                image
                    .pixels()
                    .map(|p| p.0[..3].iter().map(|c| u64::from(*c)).sum::<u64>())
                    .sum()
            };
            assert!(brightness(&light) > brightness(&dark));
            assert_eq!(light.dimensions(), dark.dimensions());
            // Raster output is BGRA; the warm source remains warm on light paper.
            assert!(light.pixels().all(|p| p.0[2] >= p.0[1] && p.0[1] >= p.0[0]));
        }
    }

    #[gpui::test]
    fn appearance_changes_cache_both_variants_and_share_unchanged_dither(
        cx: &mut gpui::TestAppContext,
    ) {
        let source = fixture();
        for effect in [
            NewThreadBackgroundEffect::Scanlines,
            NewThreadBackgroundEffect::Dither,
            NewThreadBackgroundEffect::Grain,
            NewThreadBackgroundEffect::Vignette,
        ] {
            cx.update(|cx| {
                source.raster_image(effect, false, cx);
                source.raster_image(effect, true, cx);
            });
            cx.run_until_parked();
            cx.update(|cx| {
                let dark = source.raster_image(effect, false, cx).unwrap();
                let light = source.raster_image(effect, true, cx).unwrap();
                assert_eq!(
                    Arc::ptr_eq(&dark, &light),
                    matches!(
                        effect,
                        NewThreadBackgroundEffect::Dither | NewThreadBackgroundEffect::Grain
                    )
                );
                for _ in 0..100 {
                    assert!(Arc::ptr_eq(
                        &dark,
                        &source.raster_image(effect, false, cx).unwrap()
                    ));
                    assert!(Arc::ptr_eq(
                        &light,
                        &source.raster_image(effect, true, cx).unwrap()
                    ));
                }
            });
        }
        assert_eq!(source.effects.lock().unwrap().len(), 6);
    }

    #[test]
    fn raster_treatments_preserve_source_dimensions_and_alpha() {
        let source = fixture();
        for image in [
            source.dither_pixels(),
            source.grain_pixels(),
            source.vignette_pixels(false),
            source.scanline_pixels(false),
            source.scanline_pixels(true),
        ] {
            assert_eq!(image.dimensions(), (60, 32));
            assert!(image.pixels().all(|pixel| pixel.0[3] == 200));
        }
    }
}
