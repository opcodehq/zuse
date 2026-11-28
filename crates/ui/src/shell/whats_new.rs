//! In-app "What's new": a sidebar card for the running release until it is
//! dismissed, and a dialog that pages through the changelog bundled into the
//! binary ([`zuse_update::changelog`]).
//!
//! Releases with hand-written notes (`assets/whats-new.json`) open on a stage:
//! the headline feature drawn with the app's own surfaces, text and controls,
//! rising from the bottom edge of a grain field in the release's tint. The summary is the headline, followed by the highlights. Releases
//! without notes skip the stage and list their changelog bullets instead.
//!
//! Every release adds its notes when the version is bumped; a test fails
//! until the workspace version has an entry with a known scene (see
//! "Preparing a release" in `internal-docs/packaging.md`).

use super::*;
use crate::theme::mix;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::OnceLock;
use zuse_update::changelog::{self, ChangeKind, Release};

/// Website changelog with every release.
const ALL_RELEASES_URL: &str = "https://www.zuse.sh/changelog";
/// Changelog bullets listed for releases without hand-written notes.
const MAX_FALLBACK_HIGHLIGHTS: usize = 4;
/// Widest the dialog gets; narrower windows shrink it to fit.
const DIALOG_WIDTH: f32 = 520.0;
/// Dialog width below which the footer drops its "All releases" link.
const ROOMY_FOOTER_WIDTH: f32 = 460.0;
/// Space kept between the dialog and the window edges.
const DIALOG_MARGIN: f32 = 16.0;
/// Stage height at full scale.
const STAGE_HEIGHT: f32 = 248.0;
/// Smallest scale the stage's scene shrinks to in a narrow dialog.
const MIN_STAGE_SCALE: f32 = 0.62;
/// Inset shared by the dialog's copy and its footer band.
const INSET: f32 = 20.0;
const CARD_THUMB_HEIGHT: f32 = 96.0;
/// The compact sidebar card draws every scene at this fraction of full size.
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
        "laptop" => icons::LAPTOP,
        "folder" => icons::FOLDER_WITH_FILES,
        "activity" => icons::CLOUD_ACTIVITY,
        "history" => icons::HISTORY,
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
    /// Hand-written highlights carry a short title worth setting in medium
    /// weight; changelog bullets are full sentences and read as prose.
    headline: bool,
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
                headline: true,
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
                headline: false,
            })
            .collect(),
    }
}

/// The dialog's width and its stage's scale for a window `viewport_width`
/// wide: full size when there is room, shrinking with the window after that.
fn dialog_layout(viewport_width: f32) -> (f32, f32) {
    let width = DIALOG_WIDTH
        .min(viewport_width - DIALOG_MARGIN * 2.0)
        .max(0.0);
    let scale = (width / DIALOG_WIDTH).clamp(MIN_STAGE_SCALE, 1.0);
    (width, scale)
}

// ---------------------------------------------------------------------------
// Stage tint
// ---------------------------------------------------------------------------

/// Hue of the stage's grain. Olive is the send key's own colour.
struct Tint {
    name: &'static str,
    rgb: [u8; 3],
}

const TINTS: [Tint; 4] = [
    Tint {
        name: "olive",
        rgb: popover::DITHER_KEY_RGB,
    },
    Tint {
        name: "teal",
        rgb: [86, 170, 160],
    },
    Tint {
        name: "slate",
        rgb: [128, 142, 196],
    },
    Tint {
        name: "sand",
        rgb: [200, 162, 108],
    },
];

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

/// The wash colour: olive follows the primary key (so an accent theme's keys
/// and stage agree), the other tints are fixed hues.
fn tint_ink(release: &Release, theme: &Theme) -> gpui::Hsla {
    let tint = &TINTS[tint_index(release)];
    if tint.name == "olive" {
        return popover::dither_ink(theme);
    }
    let [r, g, b] = tint.rgb;
    gpui::Rgba {
        r: f32::from(r) / 255.0,
        g: f32::from(g) / 255.0,
        b: f32::from(b) / 255.0,
        a: 1.0,
    }
    .into()
}

// ---------------------------------------------------------------------------
// Scenes: the headline feature, drawn with the app's own surfaces
// ---------------------------------------------------------------------------

/// Scene names usable in `whats-new.json`; each needs an arm in [`stage`].
#[cfg(test)]
pub(super) const SCENES: [&str; 3] = ["update-channel", "model-picker", "computer-approval"];

/// Sizes scaled for the dialog (`k = 1`), a narrow dialog, or the sidebar card.
#[derive(Clone, Copy)]
struct Scale(f32);

impl Scale {
    fn px(self, value: f32) -> Pixels {
        px(value * self.0)
    }
}

/// An app surface in the scene: an opaque panel, framed like a window.
fn scene_panel(theme: &Theme, k: Scale, width: f32, radius: f32) -> gpui::Div {
    div()
        .w(k.px(width))
        .flex()
        .flex_col()
        .rounded(k.px(radius))
        .border_1()
        .border_color(theme.border_strong)
        .bg(theme.surface_dialog)
        .shadow_md()
        .text_color(theme.text)
        .text_size(k.px(13.0))
}

/// A floating menu in the scene. Overlaps text, so it paints in its own
/// layer (see `frost::layered`).
fn scene_menu(theme: &Theme, k: Scale, width: f32) -> gpui::Div {
    scene_panel(theme, k, width, 9.0)
        .p(k.px(4.0))
        .shadow_lg()
        .text_size(k.px(12.0))
}

/// A menu row; the selected one carries the accent fill and a check.
fn scene_option(theme: &Theme, k: Scale, selected: bool) -> gpui::Div {
    div()
        .px(k.px(8.0))
        .py(k.px(5.0))
        .rounded(k.px(6.0))
        .flex()
        .items_center()
        .gap(k.px(8.0))
        .when(selected, |s| {
            s.bg(theme.accent_fill()).text_color(theme.text_strong)
        })
}

fn scene_check(theme: &Theme, k: Scale) -> gpui::Svg {
    icon(icons::CHECK)
        .size(k.px(12.0))
        .text_color(theme.text_strong)
}

/// A crop of the Settings window: the App sidebar on General, and the About
/// Zuse card with the update channel menu open on Preview. Page labels are
/// the settings page's own strings; the sidebar mirrors its section list.
fn update_channel_scene(theme: &Theme, k: Scale, cx: &App) -> AnyElement {
    let general = tr(cx, msg::settings::settings_page_general);
    let nav_item = |glyph: &'static str, label: SharedString, selected: bool| {
        div()
            .px(k.px(8.0))
            .py(k.px(5.0))
            .flex()
            .items_center()
            .gap(k.px(8.0))
            .rounded(k.px(6.0))
            .when(selected, |s| {
                s.bg(theme.accent_fill()).text_color(theme.text_strong)
            })
            .child(
                icon(glyph)
                    .flex_none()
                    .size(k.px(13.0))
                    .text_color(if selected {
                        theme.text_strong
                    } else {
                        theme.text_muted
                    }),
            )
            .child(div().min_w_0().truncate().child(label))
    };
    let nav = div()
        .flex_none()
        .w(k.px(156.0))
        .px(k.px(7.0))
        .py(k.px(10.0))
        .flex()
        .flex_col()
        .bg(mix(theme.surface_dialog, theme.surface, 0.5))
        .border_r_1()
        .border_color(theme.border)
        .child(
            div()
                .px(k.px(8.0))
                .pt(k.px(4.0))
                .pb(k.px(5.0))
                .text_size(k.px(11.0))
                .text_color(theme.text_muted)
                .child("App"),
        )
        .child(nav_item(icons::DITHER_GEAR, general.clone().into(), true))
        .children(
            [
                (icons::DITHER_PEN, "Appearance"),
                (icons::DITHER_BELL, "Notifications"),
                (icons::DITHER_MIC, "Voice"),
                (icons::DITHER_KEYBOARD, "Keyboard shortcuts"),
                (icons::DITHER_FOLDER, "Files"),
                (icons::DITHER_MONITOR, "Remote access"),
            ]
            .into_iter()
            .map(|(glyph, label)| nav_item(glyph, label.into(), false)),
        );

    let preview = tr(cx, msg::settings::update_channel_preview);
    let select = div()
        .flex_none()
        .w(k.px(96.0))
        .px(k.px(9.0))
        .py(k.px(5.0))
        .flex()
        .items_center()
        .justify_between()
        .rounded(k.px(7.0))
        .border_1()
        .border_color(theme.border_strong)
        .bg(theme.surface_dialog)
        .text_color(theme.text_strong)
        .child(preview.clone())
        .child(
            icon(icons::ALT_ARROW_DOWN)
                .size(k.px(11.0))
                .text_color(theme.text_muted),
        );
    let menu = div()
        .absolute()
        .top(k.px(46.0))
        .right(k.px(11.0))
        .child(crate::frost::layered(
            scene_menu(theme, k, 96.0)
                .child(
                    scene_option(theme, k, false)
                        .child(tr(cx, msg::settings::update_channel_stable)),
                )
                .child(
                    scene_option(theme, k, true)
                        .justify_between()
                        .child(preview)
                        .child(scene_check(theme, k)),
                ),
        ));
    let channel_row = div()
        .relative()
        .px(k.px(11.0))
        .py(k.px(9.0))
        .flex()
        .items_start()
        .gap(k.px(10.0))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(k.px(2.0))
                .child(
                    div()
                        .truncate()
                        .child(tr(cx, msg::settings::update_channel_label)),
                )
                .child(
                    div()
                        .line_clamp(2)
                        .text_size(k.px(11.0))
                        .line_height(k.px(15.0))
                        .text_color(theme.text_muted)
                        .child(tr(cx, msg::native::release_channel_description)),
                ),
        )
        .child(select)
        .child(menu);
    let page = div()
        .flex_1()
        .min_w_0()
        .px(k.px(14.0))
        .pt(k.px(14.0))
        .flex()
        .flex_col()
        .child(
            div()
                .text_size(k.px(14.0))
                .font_weight(gpui::FontWeight::MEDIUM)
                .text_color(theme.text_strong)
                .child(general),
        )
        .child(
            div()
                .px(k.px(2.0))
                .pt(k.px(12.0))
                .pb(k.px(5.0))
                .text_size(k.px(11.0))
                .text_color(theme.text_muted)
                .child(tr(cx, msg::native::about_zuse)),
        )
        .child(
            div()
                .rounded(k.px(10.0))
                .border_1()
                .border_color(theme.border)
                .bg(theme.ink(0.03))
                .child(channel_row),
        );

    scene_panel(theme, k, 468.0, 12.0)
        // Taller than the stage: the window runs on past the bottom edge.
        .min_h(k.px(300.0))
        .flex_row()
        .overflow_hidden()
        .text_size(k.px(12.0))
        .child(nav)
        .child(page)
        .into_any_element()
}

/// The composer with the model picker open on its Favorites, the send key
/// in the primary dither.
fn model_picker_scene(theme: &Theme, k: Scale) -> AnyElement {
    let star = || {
        icon(icons::STAR_BOLD)
            .size(k.px(12.0))
            .text_color(gpui::rgb(0xd9a326))
    };
    let model = |name: &'static str, detail: &'static str, selected: bool| {
        scene_option(theme, k, selected)
            .gap(k.px(9.0))
            .py(k.px(6.0))
            .child(star())
            .child(div().flex_1().child(name))
            .child(if selected {
                scene_check(theme, k).into_any_element()
            } else {
                div()
                    .text_size(k.px(11.0))
                    .text_color(theme.text_muted)
                    .child(detail)
                    .into_any_element()
            })
    };
    let picker = scene_menu(theme, k, 238.0)
        .p(k.px(5.0))
        .text_size(k.px(13.0))
        .child(
            div()
                .px(k.px(8.0))
                .pt(k.px(6.0))
                .pb(k.px(4.0))
                .text_size(k.px(11.0))
                .text_color(theme.text_muted)
                .child("Favorites"),
        )
        .child(model("Sonnet 4.6", "", true))
        .child(model("Opus 4.6", "Deep work", false))
        .child(model("GPT-5.4", "Fast", false))
        .child(
            div()
                .mx(k.px(4.0))
                .my(k.px(4.0))
                .h(px(1.0))
                .bg(theme.border),
        )
        .child(
            scene_option(theme, k, false)
                .py(k.px(6.0))
                .justify_between()
                .text_color(theme.text_muted)
                .child("All models")
                .child(
                    icon(icons::ALT_ARROW_RIGHT)
                        .size(k.px(11.0))
                        .text_color(theme.text_muted),
                ),
        );
    let chip = |glyph: gpui::Svg, label: &'static str, active: bool| {
        div()
            .flex()
            .items_center()
            .gap(k.px(6.0))
            .px(k.px(8.0))
            .py(k.px(4.0))
            .rounded(k.px(7.0))
            .text_size(k.px(12.0))
            .when(active, |s| {
                s.bg(theme.accent_fill()).text_color(theme.text_strong)
            })
            .child(glyph)
            .child(label)
            .child(
                icon(icons::ALT_ARROW_DOWN)
                    .size(k.px(10.0))
                    .text_color(theme.text_muted),
            )
    };
    let send = popover::dither_key(
        div()
            .ml_auto()
            .size(k.px(30.0))
            .rounded(k.px(8.0))
            .flex()
            .items_center()
            .justify_center(),
        theme,
        popover::DitherKey::Primary,
        8.0 * k.0,
    )
    .child(
        icon(icons::DITHER_SEND)
            .relative()
            .size(k.px(14.0))
            .text_color(gpui::white()),
    );
    let composer = scene_panel(theme, k, 436.0, 14.0)
        .px(k.px(12.0))
        .pt(k.px(12.0))
        .pb(k.px(24.0))
        .child(
            div()
                .px(k.px(4.0))
                .pt(k.px(2.0))
                .pb(k.px(14.0))
                .text_color(theme.text_faint)
                .child("Ask for a change, or @ a file"),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(k.px(6.0))
                .child(chip(star(), "Sonnet 4.6", true))
                .child(chip(
                    icon(icons::DASHBOARD_SPEED)
                        .size(k.px(12.0))
                        .text_color(theme.text_muted),
                    "Medium",
                    false,
                ))
                .child(send),
        );
    div()
        .relative()
        .w(k.px(436.0))
        .h_full()
        .child(
            div()
                .absolute()
                .left_0()
                .bottom(k.px(-14.0))
                .child(composer),
        )
        .child(
            div()
                .absolute()
                .left(k.px(12.0))
                .top(k.px(18.0))
                .child(crate::frost::layered(picker)),
        )
        .into_any_element()
}

/// Height of the computer approval card in scene units, and the space kept
/// above and below it on a short stage.
const APPROVAL_CARD_HEIGHT: f32 = 192.0;
const APPROVAL_MARGIN: f32 = 10.0;

/// The approval scene's scale on a stage `height` tall: the stage's own
/// scale, shrunk further when needed so the whole card, actions included,
/// stays on a short stage such as the sidebar card's.
fn approval_scale(k: Scale, height: f32) -> Scale {
    Scale(k.0.min(height / (APPROVAL_CARD_HEIGHT + APPROVAL_MARGIN * 2.0)))
}

/// A cloud chat asking to run a command on your own computer: the approval
/// that takes the composer's place, laid out like the real prompt
/// (`composer::computers`), under the message that asked for it.
fn computer_approval_scene(theme: &Theme, k: Scale) -> AnyElement {
    let ghost = |label: &'static str| {
        div()
            .flex_none()
            .h(k.px(28.0))
            .px(k.px(8.0))
            .flex()
            .items_center()
            .rounded(k.px(6.0))
            .text_size(k.px(11.0))
            .text_color(theme.text_muted)
            .child(label)
    };
    let allow_once = popover::dither_key(
        div()
            .flex_none()
            .ml(k.px(4.0))
            .h(k.px(28.0))
            .px(k.px(12.0))
            .flex()
            .items_center()
            .rounded(k.px(8.0))
            .text_size(k.px(11.0))
            .font_weight(gpui::FontWeight::MEDIUM)
            .text_color(gpui::white()),
        theme,
        popover::DitherKey::Primary,
        8.0 * k.0,
    )
    // Above the dither fill.
    .child(div().relative().child("Allow once"));
    let header = div()
        .h(k.px(24.0))
        .flex()
        .items_center()
        .gap(k.px(8.0))
        .child(
            div()
                .flex_none()
                .size(k.px(24.0))
                .flex()
                .items_center()
                .justify_center()
                .rounded(k.px(6.0))
                .bg(theme.muted_fill())
                .child(
                    icon(icons::LAPTOP)
                        .size(k.px(14.0))
                        .text_color(theme.text_muted),
                ),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_size(k.px(12.0))
                .text_color(theme.text_muted)
                .child("Computer"),
        );
    let command = div()
        .mt(k.px(10.0))
        .rounded(k.px(6.0))
        .px(k.px(10.0))
        .py(k.px(7.0))
        .bg(theme.muted_fill().opacity(0.45))
        .font_family(theme.font_mono.clone())
        .text_size(k.px(11.0))
        .line_height(k.px(16.0))
        .text_color(theme.text)
        .child(div().text_color(theme.text_muted).child("~/code/zuse"))
        .child("$ cargo test");
    let actions = div()
        .mt(k.px(10.0))
        .flex()
        .items_center()
        .gap(k.px(4.0))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_size(k.px(10.0))
                .text_color(theme.text_muted)
                .child("zsh"),
        )
        .child(ghost("Deny"))
        .child(ghost("Allow for this chat"))
        .child(ghost("Allow always"))
        .child(allow_once);
    let card = scene_panel(theme, k, 436.0, 12.0)
        .flex_none()
        .h(k.px(APPROVAL_CARD_HEIGHT))
        .p(k.px(12.0))
        .overflow_hidden()
        .child(header)
        .child(
            div()
                .mt(k.px(10.0))
                .text_size(k.px(13.0))
                .line_height(k.px(20.0))
                .font_weight(gpui::FontWeight::MEDIUM)
                .text_color(theme.text_strong)
                .child("Run this command on your computer?"),
        )
        .child(
            div()
                .truncate()
                .text_size(k.px(12.0))
                .line_height(k.px(18.0))
                .text_color(theme.text_muted)
                .child("Swaraj's MacBook"),
        )
        .child(command)
        .child(actions);
    // The message that asked for it; a short stage crops it off the top.
    let request = div()
        .flex_none()
        .self_end()
        .px(k.px(12.0))
        .py(k.px(5.0))
        .rounded(k.px(12.0))
        .border_1()
        .border_color(theme.border)
        .bg(theme.surface_dialog)
        .shadow_sm()
        .text_size(k.px(12.0))
        .line_height(k.px(18.0))
        .text_color(theme.text)
        .child("Run the tests on my Mac");
    div()
        .w(k.px(436.0))
        .h_full()
        .pb(k.px(APPROVAL_MARGIN))
        .flex()
        .flex_col()
        .justify_end()
        .gap(k.px(8.0))
        .child(request)
        .child(card)
        .into_any_element()
}

/// The release's stage, or `None` when its notes name no scene: the scene
/// rising from the bottom edge of a grain field in the release's tint. `top_radius` and `bottom_radius` are the frame's rounding, which
/// the field's cells stay inside.
fn stage(
    release: &Release,
    height: f32,
    (top_radius, bottom_radius): (f32, f32),
    k: Scale,
    theme: &Theme,
    cx: &App,
) -> Option<gpui::Div> {
    let (scene, top) = match notes(release)?.scene.as_deref()? {
        "update-channel" => (update_channel_scene(theme, k, cx), Some(26.0)),
        "model-picker" => (model_picker_scene(theme, k), None),
        "computer-approval" => (
            computer_approval_scene(theme, approval_scale(k, height)),
            None,
        ),
        _ => return None,
    };
    Some(
        div()
            .relative()
            .w_full()
            .h(px(height))
            .flex_none()
            .overflow_hidden()
            .child(announcement_background(
                theme,
                tint_ink(release, theme),
                top_radius,
                bottom_radius,
            ))
            .child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .bottom_0()
                    .top(k.px(top.unwrap_or(0.0)))
                    .flex()
                    .justify_center()
                    // Own layer: inside the frosted dialog the panels would
                    // otherwise share a draw order with the field.
                    .child(crate::frost::layered(scene)),
            ),
    )
}

/// The textured panel shared by release announcements and the skill installer.
pub(crate) fn announcement_background(
    theme: &Theme,
    ink: gpui::Hsla,
    top_radius: f32,
    bottom_radius: f32,
) -> gpui::Div {
    let tone = |sat: f32, lightness: f32| gpui::hsla(ink.h, (ink.s * sat).min(0.6), lightness, 1.0);
    // Grain reads as texture, not noise, when its tones sit close to the
    // base: a deep tinted gradient in the dark, a pale one in the light.
    let palette = if theme.appearance.is_dark() {
        FieldPalette {
            top: tone(0.78, 0.127),
            bottom: tone(0.8, 0.06),
            mid: tone(0.95, 0.19),
            high: tone(1.05, 0.29),
        }
    } else {
        FieldPalette {
            top: tone(1.15, 0.906),
            bottom: tone(1.1, 0.847),
            mid: tone(1.0, 0.804),
            high: tone(0.95, 0.657),
        }
    };
    div()
        .absolute()
        .top_0()
        .left_0()
        .right_0()
        .bottom_0()
        .rounded(px(top_radius))
        .bg(gpui::linear_gradient(
            180.0,
            gpui::linear_color_stop(palette.top, 0.0),
            gpui::linear_color_stop(palette.bottom, 1.0),
        ))
        .child(grain_field(palette, top_radius, bottom_radius))
}

/// One stage cell, in logical pixels: the send key's grain.
const FIELD_CELL: f32 = 2.0;
/// Densest the grain gets, in the frame's corners.
const FIELD_MAX_DENSITY: f32 = 0.62;

/// The stage's colours: a vertical base gradient and two grain tones.
#[derive(Clone, Copy)]
struct FieldPalette {
    top: gpui::Hsla,
    bottom: gpui::Hsla,
    mid: gpui::Hsla,
    high: gpui::Hsla,
}

/// Stable per-cell noise in `0..1`.
fn grain(x: u32, y: u32) -> f32 {
    let mut h = x.wrapping_mul(374_761_393) ^ y.wrapping_mul(668_265_263);
    h = (h ^ (h >> 13)).wrapping_mul(1_274_126_177);
    (h ^ (h >> 16)) as f32 / u32::MAX as f32
}

/// How much grain stage cell `(nx, ny)` carries, with `nx` and `ny` in
/// `-1..1` from the centre: none behind the scene, thickening toward the
/// frame's edges.
fn field_density(nx: f32, ny: f32) -> f32 {
    let r = (nx * nx * 0.62 + ny * ny * 0.55).sqrt().min(1.0);
    let t = ((r - 0.3) / 0.7).clamp(0.0, 1.0);
    FIELD_MAX_DENSITY * t * t * (3.0 - 2.0 * t)
}

/// The tone cell `(x, y)` takes at `density`, if any: the deeper tone where
/// the grain is thickest, the mid tone around it.
fn field_tone(x: u32, y: u32, density: f32, palette: FieldPalette) -> Option<gpui::Hsla> {
    let n = grain(x, y);
    if n < density * density {
        Some(palette.high)
    } else if n < density {
        Some(palette.mid)
    } else {
        None
    }
}

/// The stage's grain: 2px cells of two close tones over the base gradient,
/// densest at the frame's edges. Paints its positioned parent, skipping cells
/// outside its rounded corners.
fn grain_field(palette: FieldPalette, top_radius: f32, bottom_radius: f32) -> impl IntoElement {
    gpui::canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            let width = f32::from(bounds.size.width);
            let height = f32::from(bounds.size.height);
            if width < 1.0 || height < 1.0 {
                return;
            }
            let columns = (width / FIELD_CELL).round().max(1.0) as u32;
            let rows = (height / FIELD_CELL).round().max(1.0) as u32;
            let cell_w = width / columns as f32;
            let cell_h = height / rows as f32;
            for y in 0..rows {
                let cy = (y as f32 + 0.5) * cell_h;
                let (radius, dy) = if cy < height / 2.0 {
                    (top_radius, (top_radius - cy).max(0.0))
                } else {
                    (bottom_radius, (cy - (height - bottom_radius)).max(0.0))
                };
                for x in 0..columns {
                    let cx = (x as f32 + 0.5) * cell_w;
                    let dx = (radius - cx).max(cx - (width - radius)).max(0.0);
                    if dx * dx + dy * dy > radius * radius {
                        continue;
                    }
                    let density = field_density(cx / width * 2.0 - 1.0, cy / height * 2.0 - 1.0);
                    let Some(tone) = field_tone(x, y, density, palette) else {
                        continue;
                    };
                    window.paint_quad(gpui::fill(
                        gpui::Bounds::new(
                            bounds.origin
                                + gpui::point(px(x as f32 * cell_w), px(y as f32 * cell_h)),
                            gpui::size(px(cell_w), px(cell_h)),
                        ),
                        tone,
                    ));
                }
            }
        },
    )
    .absolute()
    .inset_0()
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
        let thumb = stage(
            release,
            CARD_THUMB_HEIGHT,
            (7.0, 7.0),
            Scale(COMPACT_SCALE),
            theme,
            cx,
        )
        .map(|s| {
            s.rounded(px(7.0))
                .border_1()
                .border_color(theme.hairline(0.08))
        });
        let has_thumb = thumb.is_some();
        let hover_bg = theme.glass_hover();
        let dismiss = div()
            .id("whats-new-card-dismiss")
            .absolute()
            .top(px(if has_thumb { 10.0 } else { 7.0 }))
            .right(px(if has_thumb { 10.0 } else { 7.0 }))
            .size(px(18.0))
            .rounded_full()
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .role(gpui::Role::Button)
            .aria_label(tr(cx, msg::native::whats_new_dismiss))
            .on_click(cx.listener(|this, _, _, cx| {
                cx.stop_propagation();
                this.mark_whats_new_seen(cx);
            }))
            .tip(tr(cx, msg::native::whats_new_dismiss));
        // Over a scene the button needs its own plate; on the bare card it
        // is a quiet glyph like the sidebar's other controls.
        let dismiss = if has_thumb {
            dismiss
                .bg(gpui::black().opacity(0.45))
                .hover(|s| s.bg(gpui::black().opacity(0.75)))
                .child(icon(icons::CLOSE).size(px(9.0)).text_color(gpui::white()))
        } else {
            let wash = theme.ink(0.08);
            dismiss.hover(move |s| s.bg(wash)).child(
                icon(icons::CLOSE)
                    .size(px(9.0))
                    .text_color(theme.text_muted),
            )
        };
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
                .role(gpui::Role::Button)
                .aria_label(tr(cx, |l| msg::native::whats_new_open(l, version.as_str())))
                .tip(tr(cx, |l| msg::native::whats_new_open(l, version.as_str())))
                .on_click(cx.listener(move |this, _, _, cx| this.open_whats_new(index, cx)))
                .children(thumb)
                .child(
                    div()
                        .px(px(4.0))
                        .pt(px(if has_thumb { 0.0 } else { 2.0 }))
                        .pb(px(2.0))
                        .when(!has_thumb, |s| s.pr(px(24.0)))
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
                .child(crate::frost::layered(dismiss))
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
        let title = tr(cx, |l| msg::native::whats_new_title(l, version.as_str()));
        let (width, scale) = dialog_layout(f32::from(viewport.width));

        let stage = stage(
            release,
            STAGE_HEIGHT * scale,
            (15.0, 0.0),
            Scale(scale),
            &theme,
            cx,
        )
        .map(|s| {
            s.rounded_t(px(15.0))
                .border_b_1()
                .border_color(theme.hairline(0.08))
        });
        let has_stage = stage.is_some();

        // Notes lead with their summary and name the release above it; a
        // bare release is its own headline, with its change counts above.
        let (meta, headline) = match notes {
            Some(notes) => (
                div()
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(theme.text)
                    .child(title.clone()),
                notes.summary.clone(),
            ),
            None => {
                let counts = release.counts();
                (
                    div().child(tr(cx, |l| {
                        msg::native::whats_new_summary(
                            l,
                            counts.added,
                            counts.changed,
                            counts.fixed,
                        )
                    })),
                    title.clone(),
                )
            }
        };
        let meta = div()
            .flex()
            .flex_wrap()
            .items_baseline()
            .gap_x(px(6.0))
            .text_size(crate::typography::text_xs())
            .text_color(theme.text_muted)
            .child(meta)
            .children(release.date.as_deref().map(|date| {
                div()
                    .flex()
                    .gap_x(px(6.0))
                    .child("·")
                    .child(release_date(date, cx))
            }));
        let headline = div()
            .mt(px(4.0))
            .text_size(crate::typography::text_xl())
            .line_height(px(24.0))
            .font_weight(gpui::FontWeight::MEDIUM)
            .text_color(theme.text_strong)
            .child(headline);

        let list = div()
            .mt(px(18.0))
            .flex()
            .flex_col()
            .gap(px(14.0))
            .children(rows.iter().map(|row| highlight_row(&theme, row)));

        let more = (hidden > 0).then(|| {
            let url = format!("{ALL_RELEASES_URL}#v{version}");
            let label = tr(cx, |l| msg::native::whats_new_more_plural(l, hidden));
            div()
                .id("whats-new-more")
                .mt(px(14.0))
                .ml(px(HIGHLIGHT_INDENT))
                .text_size(crate::typography::text_xs())
                .text_color(theme.text_muted)
                .cursor_pointer()
                .hover(|s| s.underline())
                .role(gpui::Role::Link)
                .aria_label(label.clone())
                .child(label)
                .on_click(move |_, _, cx| cx.open_url(&url))
        });

        let body = div()
            .id("whats-new-body")
            .flex_shrink(1.0)
            .min_h_0()
            .overflow_y_scroll()
            .children(stage)
            .child(
                div()
                    .px(px(INSET))
                    .pt(px(if has_stage { 18.0 } else { INSET }))
                    .pb(px(INSET))
                    .child(meta)
                    .child(headline)
                    .child(list)
                    .children(more),
            );

        let footer =
            self.whats_new_footer(&theme, index, older, newer, width >= ROOMY_FOOTER_WIDTH, cx);
        let card = popover::dialog_card(&theme)
            .w(px(width))
            .p_0()
            .max_h((viewport.height - px(40.0)).max(px(0.0)))
            .child(body)
            .child(footer)
            .into_any_element();
        Some(popover::modal("whats-new-dialog", viewport, card))
    }

    /// The dialog's action band: this release's full notes and, with room,
    /// every release on the left; then paging through releases and the
    /// primary "Got it".
    fn whats_new_footer(
        &self,
        theme: &Theme,
        index: usize,
        older: Option<String>,
        newer: Option<usize>,
        roomy: bool,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let muted = theme.text_muted;
        let strong = theme.text;
        let wash = theme.ink(0.06);
        let full_label = tr(cx, msg::native::whats_new_full_release);
        let full_url = format!("{ALL_RELEASES_URL}#v{}", releases()[index].version);
        let full_release = div()
            .id("whats-new-full-release")
            .flex_none()
            .flex()
            .items_center()
            .gap(px(6.0))
            .px(px(12.0))
            .py(px(5.0))
            .rounded(px(8.0))
            .border_1()
            .border_color(theme.border_strong)
            .text_size(crate::typography::text_sm())
            .text_color(strong)
            .cursor_pointer()
            .hover(move |s| s.bg(wash))
            .role(gpui::Role::Link)
            .aria_label(full_label.clone())
            .on_click(move |_, _, cx| cx.open_url(&full_url))
            .child(full_label)
            .child(icon(icons::ARROW_UP_RIGHT).size(px(11.0)).text_color(muted));
        let all_label = tr(cx, msg::native::whats_new_all_releases);
        let all_releases = div()
            .id("whats-new-all")
            .flex()
            .items_center()
            .gap(px(4.0))
            .py(px(5.0))
            .text_size(crate::typography::text_xs())
            .text_color(muted)
            .cursor_pointer()
            .hover(move |s| s.text_color(strong))
            .role(gpui::Role::Link)
            .aria_label(all_label.clone())
            .on_click(|_, _, cx| cx.open_url(ALL_RELEASES_URL))
            .child(all_label)
            .child(icon(icons::ARROW_UP_RIGHT).size(px(10.0)).text_color(muted));

        let previous = tr(cx, msg::native::whats_new_previous);
        let next = tr(cx, msg::native::whats_new_next);
        let older = older.map(|v| (SharedString::from(format!("{previous} · {v}")), index + 1));
        let newer = newer.map(|i| {
            let label = format!("{next} · {}", releases()[i].version);
            (SharedString::from(label), i)
        });
        let pager = div()
            .flex()
            .items_center()
            .gap(px(2.0))
            .child(page_key(
                theme,
                "whats-new-older",
                icons::ALT_ARROW_LEFT,
                older,
                cx,
            ))
            .child(page_key(
                theme,
                "whats-new-newer",
                icons::ALT_ARROW_RIGHT,
                newer,
                cx,
            ));

        let got_it = popover::btn_primary(theme, &tr(cx, msg::native::whats_new_got_it))
            .id("whats-new-got-it")
            .px(px(14.0))
            .role(gpui::Role::Button)
            .on_click(cx.listener(|this, _, _, cx| {
                this.whats_new = None;
                this.mark_whats_new_seen(cx);
            }));

        popover::dialog_footer(theme)
            .m_0()
            .px(px(INSET))
            .flex_none()
            .justify_between()
            .child(
                div()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap(px(12.0))
                    .child(full_release)
                    // A narrow dialog keeps the release's own notes and
                    // drops the index link, so the keys stay on one line.
                    .when(roomy, |s| s.child(all_releases)),
            )
            .child(
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(pager)
                    .child(got_it),
            )
    }
}

/// One pager key. `target` is its tooltip and the release it opens; at the
/// end of the list the key stays in place, dimmed, so the others don't shift.
fn page_key(
    theme: &Theme,
    id: &'static str,
    glyph: &'static str,
    target: Option<(SharedString, usize)>,
    cx: &mut Context<Shell>,
) -> AnyElement {
    let base = div()
        .size(px(28.0))
        .rounded(px(7.0))
        .flex()
        .items_center()
        .justify_center();
    match target {
        Some((label, index)) => {
            let wash = theme.ink(0.07);
            base.id(id)
                .cursor_pointer()
                .hover(move |s| s.bg(wash))
                .role(gpui::Role::Button)
                .aria_label(label.clone())
                .tip(label)
                .on_click(cx.listener(move |this, _, _, cx| this.open_whats_new(index, cx)))
                .child(icon(glyph).size(px(13.0)).text_color(theme.text))
                .into_any_element()
        }
        None => base
            .opacity(0.35)
            .child(icon(glyph).size(px(13.0)).text_color(theme.text_muted))
            .into_any_element(),
    }
}

/// Icon column plus gap, so continuation lines align under the titles.
const HIGHLIGHT_INDENT: f32 = 27.0;
/// Highlight glyph size.
const HIGHLIGHT_ICON: f32 = 15.0;

/// One highlight: its glyph, then the title and description.
fn highlight_row(theme: &Theme, row: &Row) -> gpui::Div {
    // Body copy sits between primary and muted text: readable at length,
    // still a step below the titles.
    let body_color = if row.headline {
        mix(theme.text, theme.text_muted, 0.45)
    } else {
        theme.text_muted
    };
    div()
        .flex()
        .items_start()
        .gap(px(HIGHLIGHT_INDENT - HIGHLIGHT_ICON))
        .child(
            icon(row.icon)
                .flex_none()
                .size(px(HIGHLIGHT_ICON))
                .mt(px(2.0))
                .text_color(theme.text_muted),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(1.0))
                .text_size(crate::typography::text_sm())
                .line_height(px(19.0))
                .child(
                    div()
                        .line_clamp(3)
                        .when(row.headline, |s| {
                            s.font_weight(gpui::FontWeight::MEDIUM)
                                .text_color(theme.text_strong)
                        })
                        .when(!row.headline, |s| s.text_color(theme.text))
                        .child(row.title.clone()),
                )
                .children(row.body.clone().map(|body| {
                    div()
                        .line_clamp(if row.headline { 4 } else { 3 })
                        .text_color(body_color)
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

    /// Release guard: the version being built and the newest changelog
    /// section must ship hand-written notes with a stage, so the dialog never
    /// opens on bare changelog bullets. Add the entry to
    /// `assets/whats-new.json` when bumping the version (see
    /// `internal-docs/packaging.md`).
    #[test]
    fn the_release_being_built_has_whats_new_notes_with_a_scene() {
        let newest = releases().first().expect("CHANGELOG.md has a release");
        // This crate takes the workspace version from the root Cargo.toml.
        for version in [env!("CARGO_PKG_VERSION").to_owned(), newest.version.clone()] {
            let Some(n) = all_notes().get(&version) else {
                panic!("add a {version} entry to crates/ui/assets/whats-new.json");
            };
            assert!(!n.summary.trim().is_empty(), "{version}: empty summary");
            assert!(
                (2..=4).contains(&n.highlights.len()),
                "{version}: needs 2 to 4 highlights"
            );
            for h in &n.highlights {
                assert!(
                    named_icon(&h.icon).is_some(),
                    "{version}: unknown icon {}",
                    h.icon
                );
                assert!(
                    !h.title.trim().is_empty() && !h.body.trim().is_empty(),
                    "{version}: highlight without a title or body"
                );
            }
            let scene = n.scene.as_deref().unwrap_or_default();
            assert!(
                SCENES.contains(&scene),
                "{version}: scene {scene:?} is not one of {SCENES:?}"
            );
        }
    }

    #[test]
    fn approval_scene_keeps_the_whole_card_on_short_stages() {
        assert_eq!(approval_scale(Scale(1.0), STAGE_HEIGHT).0, 1.0);
        let narrow = STAGE_HEIGHT * MIN_STAGE_SCALE;
        assert_eq!(
            approval_scale(Scale(MIN_STAGE_SCALE), narrow).0,
            MIN_STAGE_SCALE
        );
        let compact = approval_scale(Scale(COMPACT_SCALE), CARD_THUMB_HEIGHT).0;
        assert!(compact < COMPACT_SCALE);
        assert!(
            (APPROVAL_CARD_HEIGHT + APPROVAL_MARGIN) * compact <= CARD_THUMB_HEIGHT,
            "card overflows the sidebar thumbnail"
        );
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
        assert!(rows.iter().all(|row| !row.headline));
    }

    #[test]
    fn dialog_shrinks_with_narrow_windows() {
        assert_eq!(dialog_layout(1280.0), (DIALOG_WIDTH, 1.0));
        let (width, scale) = dialog_layout(420.0);
        assert_eq!(width, 420.0 - DIALOG_MARGIN * 2.0);
        assert!((scale - width / DIALOG_WIDTH).abs() < f32::EPSILON);
        assert_eq!(dialog_layout(200.0).1, MIN_STAGE_SCALE);
        assert_eq!(dialog_layout(10.0).0, 0.0);
    }

    #[test]
    fn stage_grain_is_clear_behind_the_scene_and_thickens_at_the_edges() {
        assert_eq!(field_density(0.0, 0.0), 0.0);
        let corner = field_density(1.0, -1.0);
        assert!((corner - FIELD_MAX_DENSITY).abs() < 1e-4, "corner {corner}");
        assert!(field_density(0.9, 0.0) > field_density(0.6, 0.0));
        // Never solid, so the base gradient always shows through.
        let black = gpui::hsla(0.0, 0.0, 0.0, 1.0);
        let palette = FieldPalette {
            top: black,
            bottom: black,
            mid: black,
            high: black,
        };
        let lit = (0..64u32)
            .flat_map(|x| (0..64u32).map(move |y| (x, y)))
            .filter(|&(x, y)| field_tone(x, y, corner, palette).is_some())
            .count();
        assert!(lit > 64 * 64 / 2 && lit < 64 * 64 * 3 / 4, "lit {lit}");
        assert_eq!(grain(3, 7), grain(3, 7));
    }

    #[test]
    fn every_bundled_release_has_a_tint() {
        assert!(releases().iter().all(|r| tint_index(r) < TINTS.len()));
    }
}
