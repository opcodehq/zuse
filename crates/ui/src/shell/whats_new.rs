//! In-app "What's new": a sidebar card for the running release until it is
//! dismissed, and a dialog that pages through the changelog bundled into the
//! binary ([`zuse_update::changelog`]).
//!
//! Releases with hand-written notes (`assets/whats-new.json`) get a summary,
//! three short highlights with their own icons, and a hero "scene": a small,
//! simplified mock of the feature drawn with real UI elements on a dithered
//! backdrop. Releases without notes fall back to the changelog bullets and a
//! plain version card.

use super::*;
use crate::theme::{hairline, ink};
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::{Arc, OnceLock};
use zuse_update::changelog::{self, ChangeKind, Release};

/// Website changelog with every release.
const ALL_RELEASES_URL: &str = "https://www.zuse.sh/changelog";
/// Changelog bullets listed for releases without hand-written notes.
const MAX_FALLBACK_HIGHLIGHTS: usize = 4;
const DIALOG_WIDTH: f32 = 480.0;
const HERO_HEIGHT: f32 = 260.0;
const CARD_THUMB_HEIGHT: f32 = 96.0;
/// The compact sidebar hero draws every scene at this fraction of full size.
const COMPACT_SCALE: f32 = 0.56;

// ---------------------------------------------------------------------------
// Data
// ---------------------------------------------------------------------------

/// Bundled releases, newest first.
pub(super) fn releases() -> &'static [Release] {
    static RELEASES: OnceLock<Vec<Release>> = OnceLock::new();
    RELEASES.get_or_init(changelog::bundled)
}

/// The bundled release matching the running version, if the changelog has it.
fn current_index() -> Option<usize> {
    let version = zuse_update::current_version();
    releases().iter().position(|r| r.version == version)
}

/// Hand-written notes for one release.
#[derive(Debug, Deserialize)]
pub(super) struct Notes {
    pub summary: String,
    #[serde(default)]
    pub scene: Option<String>,
    #[serde(default)]
    pub tint: Option<String>,
    pub highlights: Vec<NoteHighlight>,
}

#[derive(Debug, Deserialize)]
pub(super) struct NoteHighlight {
    pub icon: String,
    pub title: String,
    pub body: String,
}

const NOTES_JSON: &str = include_str!("../../assets/whats-new.json");

pub(super) fn all_notes() -> &'static HashMap<String, Notes> {
    static NOTES: OnceLock<HashMap<String, Notes>> = OnceLock::new();
    NOTES.get_or_init(|| match serde_json::from_str(NOTES_JSON) {
        Ok(notes) => notes,
        Err(error) => {
            tracing::warn!(%error, "invalid whats-new.json; using changelog bullets");
            HashMap::new()
        }
    })
}

fn notes(release: &Release) -> Option<&'static Notes> {
    all_notes().get(&release.version)
}

/// Icon names usable in `whats-new.json`.
pub(super) fn named_icon(name: &str) -> Option<&'static str> {
    Some(match name {
        "refresh" => icons::REFRESH,
        "checklist" => icons::CHECKLIST,
        "terminal" => icons::TERMINAL,
        "star" => icons::STAR,
        "globe" => icons::GLOBE,
        "pull-request" => icons::PULL_REQUEST,
        "settings" => icons::SETTINGS_MINIMALISTIC,
        "chat" => icons::CHAT_ROUND_LINE,
        "agent" | "bot" => icons::AGENT,
        "monitor" => icons::MONITOR,
        "keyboard" => icons::KEYBOARD,
        "branch" => icons::GIT_BRANCH,
        "cloud" => icons::CLOUD,
        "bell" => icons::BELL,
        "bolt" => icons::FAST_TIER,
        "wrench" => icons::PR_REPAIR,
        "sparkle" => icons::MAGIC_STICK_3,
        _ => return None,
    })
}

fn kind_icon(kind: ChangeKind) -> &'static str {
    match kind {
        ChangeKind::Added => icons::MAGIC_STICK_3,
        ChangeKind::Changed => icons::REFRESH,
        ChangeKind::Fixed => icons::PR_REPAIR,
        ChangeKind::Other => icons::STAR,
    }
}

/// `2026-11-15` in the interface language, or the raw string if it doesn't parse.
fn release_date(date: &str, cx: &App) -> String {
    let mut parts = date.split('-');
    let parsed = (|| {
        let year = parts.next()?.parse().ok()?;
        let month = parts.next()?.parse().ok()?;
        let day = parts.next()?.parse().ok()?;
        crate::localization::localizer(cx).date(year, month, day)
    })();
    parsed.unwrap_or_else(|| date.to_owned())
}

/// One row of the dialog's highlight list.
struct Row {
    icon: &'static str,
    title: String,
    body: Option<String>,
}

/// Hand-written highlights when the release has them, else changelog bullets.
fn rows(release: &Release) -> Vec<Row> {
    match notes(release) {
        Some(notes) => notes
            .highlights
            .iter()
            .map(|h| Row {
                icon: named_icon(&h.icon).unwrap_or(icons::STAR),
                title: h.title.clone(),
                body: Some(h.body.clone()),
            })
            .collect(),
        None => release
            .highlights()
            .into_iter()
            .take(MAX_FALLBACK_HIGHLIGHTS)
            .map(|h| Row {
                icon: kind_icon(h.kind),
                title: h.title,
                body: h.detail,
            })
            .collect(),
    }
}

// ---------------------------------------------------------------------------
// Backdrop
// ---------------------------------------------------------------------------

/// Backdrop colours, close in value so the grain reads as texture, not noise.
struct Tint {
    name: &'static str,
    light: [u8; 3],
    mid: [u8; 3],
    dark: [u8; 3],
}

const TINTS: [Tint; 4] = [
    Tint {
        name: "olive",
        light: [226, 233, 206],
        mid: [178, 194, 140],
        dark: [112, 132, 74],
    },
    Tint {
        name: "teal",
        light: [214, 236, 231],
        mid: [150, 196, 188],
        dark: [74, 132, 126],
    },
    Tint {
        name: "slate",
        light: [224, 228, 238],
        mid: [168, 178, 204],
        dark: [96, 108, 146],
    },
    Tint {
        name: "sand",
        light: [240, 232, 218],
        mid: [206, 186, 156],
        dark: [146, 118, 86],
    },
];

/// Backdrop texture size: about 1:1 with the dialog hero on a 2x display.
const BACKDROP_SIZE: (u32, u32) = (900, 500);

/// The release's tint: named in its notes, else stable per version.
pub(super) fn tint_index(release: &Release) -> usize {
    if let Some(index) = notes(release)
        .and_then(|n| n.tint.as_deref())
        .and_then(|name| TINTS.iter().position(|t| t.name == name))
    {
        return index;
    }
    let hash = release
        .version
        .bytes()
        .fold(2166136261u32, |h, b| (h ^ b as u32).wrapping_mul(16777619));
    hash as usize % TINTS.len()
}

/// Stable per-pixel noise in `0..1`.
fn grain(x: u32, y: u32) -> f32 {
    let mut h = x.wrapping_mul(374_761_393) ^ y.wrapping_mul(668_265_263);
    h = (h ^ (h >> 13)).wrapping_mul(1_274_126_177);
    (h ^ (h >> 16)) as f32 / u32::MAX as f32
}

/// Three-tone stochastic dither: a clear light centre that grains into the
/// mid and dark tones toward the edges. RenderImage consumes BGRA.
pub(super) fn backdrop_pixels(tint: usize, width: u32, height: u32) -> image::RgbaImage {
    let Tint {
        light, mid, dark, ..
    } = &TINTS[tint];
    image::RgbaImage::from_fn(width, height, |x, y| {
        let nx = (x as f32 + 0.5) / width as f32 * 2.0 - 1.0;
        let ny = (y as f32 + 0.5) / height as f32 * 2.0 - 1.0;
        let distance = ((nx * nx * 0.62 + ny * ny).sqrt() / 1.27).clamp(0.0, 1.0);
        let t = ((distance - 0.28) / 0.72).clamp(0.0, 1.0);
        let density = t * t * (3.0 - 2.0 * t);
        let n = grain(x, y);
        let [r, g, b] = if n < density * density {
            *dark
        } else if n < density {
            *mid
        } else {
            *light
        };
        image::Rgba([b, g, r, 255])
    })
}

fn backdrop(tint: usize) -> Arc<gpui::RenderImage> {
    static CACHE: [OnceLock<Arc<gpui::RenderImage>>; TINTS.len()] =
        [const { OnceLock::new() }; TINTS.len()];
    CACHE[tint]
        .get_or_init(|| {
            let (width, height) = BACKDROP_SIZE;
            Arc::new(gpui::RenderImage::new([image::Frame::new(
                backdrop_pixels(tint, width, height),
            )]))
        })
        .clone()
}

// ---------------------------------------------------------------------------
// Scenes: simplified mocks of a release's headline feature
// ---------------------------------------------------------------------------

/// Light-card palette shared by every scene.
mod ink_on_card {
    pub fn card() -> gpui::Rgba {
        gpui::rgb(0xf7f7f8)
    }
    pub fn strong() -> gpui::Rgba {
        gpui::rgb(0x1b1b1f)
    }
    pub fn muted() -> gpui::Rgba {
        gpui::rgb(0x7a7a85)
    }
    pub fn line() -> gpui::Rgba {
        gpui::rgba(0x0000_0012)
    }
    pub fn track() -> gpui::Rgba {
        gpui::rgba(0x0000_000d)
    }
    pub fn green() -> gpui::Rgba {
        gpui::rgb(0x3f8f3a)
    }
}

/// Scene names usable in `whats-new.json`.
pub(super) const SCENES: [&str; 2] = ["update-channel", "model-picker"];

/// Sizes scaled for the dialog (`k = 1`) or the sidebar thumbnail.
#[derive(Clone, Copy)]
struct Scale(f32);

impl Scale {
    fn px(self, value: f32) -> Pixels {
        px(value * self.0)
    }
}

fn scene_card(k: Scale, width: f32) -> gpui::Div {
    div()
        .w(k.px(width))
        .flex()
        .flex_col()
        .rounded(k.px(12.0))
        .bg(ink_on_card::card())
        .shadow_lg()
        .text_color(ink_on_card::strong())
        .text_size(k.px(13.0))
}

fn scene_header(k: Scale, glyph: &'static str, title: SharedString) -> gpui::Div {
    div()
        .px(k.px(14.0))
        .py(k.px(11.0))
        .flex()
        .items_center()
        .gap(k.px(8.0))
        .border_b_1()
        .border_color(ink_on_card::line())
        .child(
            icon(glyph)
                .size(k.px(13.0))
                .text_color(ink_on_card::muted()),
        )
        .child(
            div()
                .flex_1()
                .font_weight(gpui::FontWeight::MEDIUM)
                .child(title),
        )
}

fn scene_row(k: Scale, label: impl IntoElement, trailing: impl IntoElement) -> gpui::Div {
    div()
        .px(k.px(14.0))
        .py(k.px(10.0))
        .flex()
        .items_center()
        .justify_between()
        .gap(k.px(10.0))
        .child(label)
        .child(trailing)
}

fn scene_label(k: Scale, title: &str, sub: &str) -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .gap(k.px(1.0))
        .child(SharedString::from(title.to_owned()))
        .child(
            div()
                .text_size(k.px(11.0))
                .text_color(ink_on_card::muted())
                .child(SharedString::from(sub.to_owned())),
        )
}

/// General settings with the Stable / Preview track switch.
fn update_channel_scene(k: Scale, version: &str) -> gpui::Div {
    let segment = |label: &'static str, on: bool| {
        div()
            .px(k.px(10.0))
            .py(k.px(4.0))
            .rounded(k.px(6.0))
            .text_size(k.px(12.0))
            .when(on, |s| s.bg(gpui::white()).shadow_sm())
            .text_color(if on {
                ink_on_card::strong()
            } else {
                ink_on_card::muted()
            })
            .child(label)
    };
    scene_card(k, 300.0)
        .child(scene_header(
            k,
            icons::SETTINGS_MINIMALISTIC,
            "General".into(),
        ))
        .child(scene_row(
            k,
            scene_label(k, "Updates", "Release track"),
            div()
                .p(k.px(2.0))
                .flex()
                .rounded(k.px(8.0))
                .bg(ink_on_card::track())
                .child(segment("Stable", false))
                .child(segment("Preview", true)),
        ))
        .child(div().mx(k.px(14.0)).h(px(1.0)).bg(ink_on_card::line()))
        .child(scene_row(
            k,
            scene_label(k, &format!("Zuse {version}"), "Up to date"),
            icon(icons::CHECK_CIRCLE)
                .size(k.px(15.0))
                .text_color(ink_on_card::green()),
        ))
}

/// The model picker's Favorites list.
fn model_picker_scene(k: Scale) -> gpui::Div {
    let model = |name: &'static str, detail: &'static str, selected: bool| {
        div()
            .mx(k.px(6.0))
            .px(k.px(8.0))
            .py(k.px(7.0))
            .rounded(k.px(7.0))
            .flex()
            .items_center()
            .gap(k.px(9.0))
            .when(selected, |s| s.bg(ink_on_card::track()))
            .child(
                icon(icons::STAR_BOLD)
                    .size(k.px(12.0))
                    .text_color(gpui::rgb(0xd9a326)),
            )
            .child(div().flex_1().child(name))
            .child(
                div()
                    .text_size(k.px(11.0))
                    .text_color(ink_on_card::muted())
                    .child(detail),
            )
    };
    scene_card(k, 280.0)
        .pb(k.px(6.0))
        .child(scene_header(k, icons::MAGNIFER, "Favorites".into()))
        .child(div().h(k.px(5.0)))
        .child(model("Balanced", "Medium", true))
        .child(model("Quick edits", "Fast", false))
        .child(model("Deep work", "High", false))
}

/// Fallback scene: the release as a small version card.
fn version_scene(k: Scale, release: &Release, cx: &App) -> gpui::Div {
    let counts = release.counts();
    let chip = |n: usize, kind: ChangeKind| {
        div()
            .flex()
            .items_center()
            .gap(k.px(5.0))
            .px(k.px(8.0))
            .py(k.px(3.0))
            .rounded(k.px(6.0))
            .bg(ink_on_card::track())
            .text_size(k.px(11.0))
            .text_color(ink_on_card::muted())
            .child(
                icon(kind_icon(kind))
                    .size(k.px(11.0))
                    .text_color(ink_on_card::muted()),
            )
            .child(SharedString::from(n.to_string()))
    };
    scene_card(k, 240.0)
        .p(k.px(16.0))
        .gap(k.px(12.0))
        .child(
            div()
                .flex()
                .items_center()
                .gap(k.px(10.0))
                .child(
                    icon(icons::ZUSE_LOGO)
                        .size(k.px(22.0))
                        .text_color(ink_on_card::strong()),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .font_weight(gpui::FontWeight::MEDIUM)
                                .child(tr(cx, |l| {
                                    msg::native::whats_new_title(l, release.version.as_str())
                                })),
                        )
                        .children(release.date.as_deref().map(|date| {
                            div()
                                .text_size(k.px(11.0))
                                .text_color(ink_on_card::muted())
                                .child(release_date(date, cx))
                        })),
                ),
        )
        .child(
            div().flex().gap(k.px(6.0)).children(
                [
                    (counts.added, ChangeKind::Added),
                    (counts.changed, ChangeKind::Changed),
                    (counts.fixed, ChangeKind::Fixed),
                ]
                .into_iter()
                .filter(|(n, _)| *n > 0)
                .map(|(n, kind)| chip(n, kind)),
            ),
        )
}

/// The release's hero: its scene floating on the dithered backdrop.
fn hero(release: &Release, height: f32, radius: f32, k: Scale, cx: &App) -> gpui::Div {
    let scene = match notes(release).and_then(|n| n.scene.as_deref()) {
        Some("update-channel") => update_channel_scene(k, &release.version),
        Some("model-picker") => model_picker_scene(k),
        _ => version_scene(k, release, cx),
    };
    div()
        .relative()
        .w_full()
        .h(px(height))
        .flex_none()
        .rounded(px(radius))
        .overflow_hidden()
        .border_1()
        .border_color(hairline(0.10))
        .bg(ink(0.06))
        .child(
            gpui::img(backdrop(tint_index(release)))
                .absolute()
                .inset_0()
                .size_full()
                .object_fit(gpui::ObjectFit::Cover),
        )
        .child(
            div()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                // Own layer: inside the frosted dialog, quads would otherwise
                // paint under the backdrop image (see `frost::layered`).
                .child(crate::frost::layered(scene)),
        )
}

// ---------------------------------------------------------------------------
// Shell surfaces
// ---------------------------------------------------------------------------

impl Shell {
    /// Opens the dialog on the running release, or the newest one when the
    /// changelog doesn't list it (source builds ahead of a release).
    pub fn show_whats_new(&mut self, cx: &mut Context<Self>) {
        self.open_whats_new(current_index().unwrap_or(0), cx);
    }

    /// Opens the dialog on one bundled release (newest first); for fixtures.
    pub fn show_whats_new_at(&mut self, index: usize, cx: &mut Context<Self>) {
        self.open_whats_new(index, cx);
    }

    pub(super) fn open_whats_new(&mut self, index: usize, cx: &mut Context<Self>) {
        if index < releases().len() {
            self.whats_new = Some(index);
            cx.notify();
        }
    }

    /// Hides the sidebar card until a newer release ships.
    fn mark_whats_new_seen(&mut self, cx: &mut Context<Self>) {
        let version = zuse_update::current_version().to_owned();
        if self.settings.whats_new_seen_version.as_deref() != Some(version.as_str()) {
            self.settings.whats_new_seen_version = Some(version);
            self.schedule_save(cx);
        }
        cx.notify();
    }

    /// The card above the sidebar footer while the running release's notes
    /// are unseen. Clicking it opens the dialog; the close button hides it.
    pub(super) fn render_whats_new_card(
        &self,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let index = current_index()?;
        let release = &releases()[index];
        if self.settings.whats_new_seen_version.as_deref() == Some(release.version.as_str()) {
            return None;
        }
        let version = release.version.clone();
        let lead = notes(release)
            .map(|n| n.summary.clone())
            .or_else(|| release.highlights().into_iter().next().map(|h| h.title))
            .unwrap_or_default();
        let hover_bg = theme.glass_hover();
        Some(
            div()
                .id("whats-new-card")
                .relative()
                .mx(px(Theme::SPACE_SM))
                .mt(px(Theme::SPACE_XS))
                .mb(px(Theme::SPACE_XS))
                .p(px(5.0))
                .flex()
                .flex_col()
                .gap(px(6.0))
                .rounded(px(10.0))
                .border_1()
                .border_color(theme.border)
                .cursor_pointer()
                .hover(move |s| s.bg(hover_bg))
                .tip(tr(cx, |l| msg::native::whats_new_open(l, version.as_str())))
                .on_click(cx.listener(move |this, _, _, cx| this.open_whats_new(index, cx)))
                .child(hero(
                    release,
                    CARD_THUMB_HEIGHT,
                    7.0,
                    Scale(COMPACT_SCALE),
                    cx,
                ))
                .child(
                    div()
                        .px(px(4.0))
                        .pb(px(2.0))
                        .flex()
                        .flex_col()
                        .gap(px(1.0))
                        .child(
                            div()
                                .text_size(crate::typography::text_sm())
                                .font_weight(gpui::FontWeight::MEDIUM)
                                .text_color(theme.text_strong)
                                .child(tr(cx, |l| {
                                    msg::native::whats_new_title(l, release.version.as_str())
                                })),
                        )
                        .child(
                            div()
                                .truncate()
                                .text_size(crate::typography::text_xs())
                                .text_color(theme.text_muted)
                                .child(lead),
                        ),
                )
                .child(crate::frost::layered(
                    div()
                        .id("whats-new-card-dismiss")
                        .absolute()
                        .top(px(10.0))
                        .right(px(10.0))
                        .size(px(18.0))
                        .rounded_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .bg(gpui::black().opacity(0.45))
                        .cursor_pointer()
                        .hover(|s| s.bg(gpui::black().opacity(0.75)))
                        .on_click(cx.listener(|this, _, _, cx| {
                            cx.stop_propagation();
                            this.mark_whats_new_seen(cx);
                        }))
                        .tip(tr(cx, msg::native::whats_new_dismiss))
                        .child(icon(icons::CLOSE).size(px(9.0)).text_color(gpui::white())),
                ))
                .into_any_element(),
        )
    }

    pub(super) fn render_whats_new_dialog(
        &mut self,
        viewport: gpui::Size<Pixels>,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let index = self.whats_new?;
        let all = releases();
        let release = all.get(index)?;
        let theme = Theme::of(cx).for_popup();
        let rows = rows(release);
        let total = release.highlights().len();
        let notes = notes(release);
        // Hand-written highlights summarise bullets, so count them as covered.
        let hidden = total.saturating_sub(rows.len());
        let older = all.get(index + 1).map(|r| r.version.clone());
        let newer = index.checked_sub(1);
        let version = release.version.clone();

        let heading = div()
            .mt(px(16.0))
            .flex()
            .items_baseline()
            .gap(px(8.0))
            .child(
                div()
                    .text_size(crate::typography::text_lg())
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(theme.text_strong)
                    .child(tr(cx, |l| {
                        msg::native::whats_new_title(l, version.as_str())
                    })),
            )
            .children(release.date.as_deref().map(|date| {
                div()
                    .text_size(crate::typography::text_sm())
                    .text_color(theme.text_muted)
                    .child(release_date(date, cx))
            }));

        let summary_text = match notes {
            Some(notes) => notes.summary.clone(),
            None => {
                let counts = release.counts();
                tr(cx, |l| {
                    msg::native::whats_new_summary(l, counts.added, counts.changed, counts.fixed)
                })
            }
        };
        let summary = div()
            .mt(px(2.0))
            .text_size(crate::typography::text_sm())
            .text_color(theme.text_muted)
            .child(summary_text);

        let list = div()
            .mt(px(18.0))
            .flex()
            .flex_col()
            .gap(px(16.0))
            .children(rows.iter().map(|row| highlight_row(&theme, row)));

        let more = (hidden > 0).then(|| {
            let url = format!("{ALL_RELEASES_URL}#v{version}");
            div()
                .id("whats-new-more")
                .mt(px(14.0))
                .ml(px(44.0))
                .text_size(crate::typography::text_xs())
                .text_color(theme.text_muted)
                .cursor_pointer()
                .hover(|s| s.underline())
                .child(tr(cx, |l| msg::native::whats_new_more_plural(l, hidden)))
                .on_click(move |_, _, cx| cx.open_url(&url))
        });

        let got_it = popover::btn_primary(&theme, &tr(cx, msg::native::whats_new_got_it))
            .id("whats-new-got-it")
            .mt(px(20.0))
            .w_full()
            .py(px(8.0))
            .flex()
            .justify_center()
            .on_click(cx.listener(|this, _, _, cx| {
                this.whats_new = None;
                this.mark_whats_new_seen(cx);
            }));

        let nav_text = theme.text_muted;
        let nav_hover = theme.text;
        let nav_item = |id: &'static str| {
            div()
                .id(id)
                .flex()
                .items_center()
                .gap(px(4.0))
                .cursor_pointer()
                .hover(move |s| s.text_color(nav_hover))
        };
        let nav = div()
            .mt(px(12.0))
            .flex()
            .items_center()
            .justify_between()
            .text_size(crate::typography::text_xs())
            .text_color(nav_text)
            .child(div().w(px(120.0)).flex().children(older.map(|older| {
                nav_item("whats-new-older")
                    .tip(tr(cx, msg::native::whats_new_previous))
                    .on_click(cx.listener(move |this, _, _, cx| this.open_whats_new(index + 1, cx)))
                    .child(
                        icon(icons::ALT_ARROW_LEFT)
                            .size(px(11.0))
                            .text_color(nav_text),
                    )
                    .child(older)
            })))
            .child(
                nav_item("whats-new-all")
                    .on_click(|_, _, cx| cx.open_url(ALL_RELEASES_URL))
                    .child(tr(cx, msg::native::whats_new_all_releases))
                    .child(
                        icon(icons::ARROW_UP_RIGHT)
                            .size(px(11.0))
                            .text_color(nav_text),
                    ),
            )
            .child(
                div().w(px(120.0)).flex().justify_end().child(match newer {
                    Some(newer) => nav_item("whats-new-newer")
                        .on_click(cx.listener(move |this, _, _, cx| this.open_whats_new(newer, cx)))
                        .child(tr(cx, msg::native::whats_new_next))
                        .child(
                            icon(icons::ALT_ARROW_RIGHT)
                                .size(px(11.0))
                                .text_color(nav_text),
                        )
                        .into_any_element(),
                    None => div()
                        .flex()
                        .items_center()
                        .gap(px(4.0))
                        .opacity(0.4)
                        .child(tr(cx, msg::native::whats_new_next))
                        .child(
                            icon(icons::ALT_ARROW_RIGHT)
                                .size(px(11.0))
                                .text_color(nav_text),
                        )
                        .into_any_element(),
                }),
            );

        let card = popover::dialog_card(&theme)
            .w(px(DIALOG_WIDTH))
            .p(px(14.0))
            .child(hero(release, HERO_HEIGHT, 10.0, Scale(1.0), cx))
            .child(
                div()
                    .px(px(4.0))
                    .child(heading)
                    .child(summary)
                    .child(list)
                    .children(more),
            )
            .child(got_it)
            .child(nav)
            .into_any_element();
        Some(popover::modal("whats-new-dialog", viewport, card))
    }
}

/// One highlight: an icon tile, then title and description.
fn highlight_row(theme: &Theme, row: &Row) -> gpui::Div {
    div()
        .flex()
        .items_start()
        .gap(px(14.0))
        .child(
            div()
                .flex_none()
                .size(px(30.0))
                .rounded(px(8.0))
                .border_1()
                .border_color(hairline(0.10))
                .bg(ink(0.04))
                .flex()
                .items_center()
                .justify_center()
                .child(icon(row.icon).size(px(14.0)).text_color(theme.text_muted)),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(2.0))
                .child(
                    div()
                        .line_clamp(2)
                        .text_size(crate::typography::text_sm())
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .text_color(theme.text)
                        .child(row.title.clone()),
                )
                .children(row.body.clone().map(|body| {
                    div()
                        .line_clamp(3)
                        .text_size(crate::typography::text_sm())
                        .line_height(px(18.0))
                        .text_color(theme.text_muted)
                        .child(body)
                })),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notes_parse_and_name_known_releases_icons_scenes_and_tints() {
        let notes = all_notes();
        assert!(!notes.is_empty(), "whats-new.json failed to parse");
        for (version, n) in notes {
            assert!(
                releases().iter().any(|r| &r.version == version),
                "{version} is not in CHANGELOG.md"
            );
            assert!(!n.highlights.is_empty() && n.highlights.len() <= 4);
            for h in &n.highlights {
                assert!(named_icon(&h.icon).is_some(), "unknown icon {}", h.icon);
            }
            if let Some(scene) = n.scene.as_deref() {
                assert!(SCENES.contains(&scene), "unknown scene {scene}");
            }
            if let Some(tint) = n.tint.as_deref() {
                assert!(TINTS.iter().any(|t| t.name == tint), "unknown tint {tint}");
            }
        }
    }

    #[test]
    fn releases_without_notes_fall_back_to_changelog_bullets() {
        let release = changelog::parse_changelog(
            "## 9.9.9 - 2026-01-01\n\n### Added\n\n- One. More.\n- Two.\n- Three.\n- Four.\n- Five.\n",
        )
        .remove(0);
        let rows = rows(&release);
        assert_eq!(rows.len(), MAX_FALLBACK_HIGHLIGHTS);
        assert_eq!(rows[0].title, "One");
        assert_eq!(rows[0].body.as_deref(), Some("More."));
    }

    #[test]
    fn backdrop_is_clear_in_the_middle_and_grainy_at_the_edges() {
        let pixels = backdrop_pixels(0, 200, 100);
        let light = TINTS[0].light;
        let off = |x: u32, y: u32| {
            let p = pixels.get_pixel(x, y);
            [p[2], p[1], p[0]] != light
        };
        let count = |xs: std::ops::Range<u32>, ys: std::ops::Range<u32>| {
            xs.flat_map(|x| ys.clone().map(move |y| (x, y)))
                .filter(|&(x, y)| off(x, y))
                .count()
        };
        assert_eq!(count(90..110, 40..60), 0);
        assert!(count(0..20, 0..20) > 300);
        assert_eq!(pixels, backdrop_pixels(0, 200, 100));
    }

    #[test]
    fn every_bundled_release_has_a_tint() {
        assert!(releases().iter().all(|r| tint_index(r) < TINTS.len()));
    }
}
