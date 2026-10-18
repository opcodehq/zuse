//! Embedded Hugeicons controls and provider brand assets.
//! The shared icon crate selects licensed solid assets when configured and
//! bundles the free set otherwise. File-identity glyphs keep their own theme.

use std::borrow::Cow;

use gpui::{
    AssetSource, Div, Hsla, ParentElement as _, Result, SharedString, Styled as _, Svg, div, px,
    svg,
};

macro_rules! icon_assets {
    ($(($const_name:ident, $path:literal)),+ $(,)?) => {
        $(pub const $const_name: &str = concat!("icons/", $path, ".svg");)+

        /// Serves the embedded control icons to gpui's SVG renderer.
        struct ControlAssets;

        impl AssetSource for ControlAssets {
            fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
                Ok(match path {
                    $(concat!("icons/", $path, ".svg") => Some(Cow::Borrowed(
                        include_bytes!(concat!("../assets/icons/", $path, ".svg")).as_slice(),
                    )),)+
                    _ => None,
                })
            }

            fn list(&self, path: &str) -> Result<Vec<SharedString>> {
                let all = [$(concat!("icons/", $path, ".svg")),+];
                Ok(all
                    .iter()
                    .filter(|p| p.starts_with(path))
                    .map(|p| SharedString::from(*p))
                    .collect())
            }
        }
    };
}

icon_assets![
    (PROJECT_DEFAULT, "project-default"),
    (REMOTE_SERVER, "remote-server"),
    (FAST_TIER, "fast-tier"),
    (FAST_TIER_BOLD, "fast-tier-bold"),
    (MONITOR, "monitor"),
    (SUN, "sun"),
    (MOON, "moon"),
    (GLOBE, "globe"),
    (LAPTOP, "laptop"),
    (PEN_NEW_SQUARE, "pen-new-square"),
    (SORT, "sort"),
    (MORE_HORIZONTAL, "more-horizontal"),
    (SORT_VERTICAL, "sort-vertical"),
    (DRAG_HANDLE, "drag-handle"),
    (QUEUE_DRAG_HANDLE, "queue-drag-handle"),
    (QUEUE_SEND, "queue-send"),
    (QUEUE_CHECK, "queue-check"),
    (QUEUE_CLOSE, "queue-close"),
    (QUEUE_PAPERCLIP, "queue-paperclip"),
    (CLOCK_CIRCLE, "clock-circle"),
    (CALENDAR, "calendar"),
    (LIST, "list"),
    (FOLDER_WITH_FILES, "folder-with-files"),
    (FILE_TREE, "file-tree"),
    (FOLDER_TREE, "folder-tree"),
    (FLOPPY_DISK, "floppy-disk"),
    (FOLDER, "folder"),
    (FORK, "fork"),
    (GIT_BRANCH, "git-branch"),
    (PULL_REQUEST, "pull-request"),
    (WORKTREE, "worktree"),
    (PLUS, "plus"),
    (ARROW_LEFT, "arrow-left"),
    (ARROW_RIGHT, "arrow-right"),
    (ALT_ARROW_DOWN, "alt-arrow-down"),
    (ALT_ARROW_UP, "alt-arrow-up"),
    (ALT_ARROW_LEFT, "alt-arrow-left"),
    (ALT_ARROW_RIGHT, "alt-arrow-right"),
    (PULL_REQUEST_DRAFT, "pull-request-draft"),
    (CLOUD, "cloud"),
    (TAG, "tag"),
    (KEY_MINIMALISTIC, "key-minimalistic"),
    (KEYBOARD, "keyboard"),
    (ARROW_UP, "arrow-up"),
    (ARROW_DOWN, "arrow-down"),
    (ARROW_UP_RIGHT, "arrow-up-right"),
    (RETURN, "return"),
    (EXPAND_ARROWS, "expand-arrows"),
    (COLLAPSE_ARROWS, "collapse-arrows"),
    (FOLD_VERTICAL, "fold-vertical"),
    (SPLIT_COLUMNS, "split-columns"),
    (WRAP_TEXT, "wrap-text"),
    (SMARTPHONE, "smartphone"),
    (ARCHIVE_UP_MINIMALISTIC, "archive-up-minimalistic"),
    (REFRESH, "refresh"),
    (RESTART, "restart"),
    (ADD_CIRCLE, "add-circle"),
    (TUNING, "tuning"),
    (EYE, "eye"),
    (EYE_CLOSED, "eye-closed"),
    (PAPERCLIP, "paperclip"),
    (PIN, "pin"),
    (PEN, "pen"),
    (ARCHIVE_MINIMALISTIC, "archive-minimalistic"),
    (TRASH_BIN_MINIMALISTIC, "trash-bin-minimalistic"),
    (SETTINGS, "settings"),
    (SETTINGS_MINIMALISTIC, "settings-minimalistic"),
    (LOGOUT_2, "logout-2"),
    (MAGNIFER, "magnifer"),
    (PALETTE_SEARCH, "palette-search"),
    (COMMAND, "command"),
    (DOCUMENT, "document"),
    (DOCUMENT_ADD, "document-add"),
    (FILE_CODE, "file-code"),
    (FILE_STYLE, "file-style"),
    (FILE_DATA, "file-data"),
    (FILE_MARKDOWN, "file-markdown"),
    (FILE_IMAGE, "file-image"),
    (GALLERY, "gallery"),
    (GLOBAL, "global"),
    (CHECKLIST, "checklist"),
    (WIDGET, "widget"),
    (MAGIC_STICK_3, "magic-stick-3"),
    (WIFI_OFF, "wifi-off"),
    (CLOSE_CIRCLE, "close-circle"),
    (INFO_CIRCLE, "info-circle"),
    (DANGER_TRIANGLE, "danger-triangle"),
    (CHAT_ROUND_LINE, "chat-round-line"),
    (BOT, "bot"),
    (BELL, "bell"),
    (VOLUME_LOUD, "volume-loud"),
    (MICROPHONE, "microphone"),
    (MICROPHONE_OFF, "microphone-off"),
    (PHONE_HANG_UP, "phone-hang-up"),
    (TERMINAL, "terminal"),
    (CLOSE, "close"),
    (WINDOW_MINIMIZE, "window-minimize"),
    (WINDOW_MAXIMIZE, "window-maximize"),
    (WINDOW_RESTORE, "window-restore"),
    (HARD_DRIVE, "hard-drive"),
    (HOME, "home"),
    (STOP, "stop"),
    (CHECK, "check"),
    (COPY, "copy"),
    (ACTION_PLAY, "action-play"),
    (ACTION_TEST, "action-test"),
    (ACTION_LINT, "action-lint"),
    (ACTION_CONFIGURE, "action-configure"),
    (ACTION_BUILD, "action-build"),
    (ACTION_DEBUG, "action-debug"),
    (STAR, "star"),
    (STAR_BOLD, "star-bold"),
    (ZUSE_LOGO, "zuse-logo"),
    (CLAUDE_MARK, "claude-mark"),
    (OPENAI_MARK, "openai-mark"),
    (CURSOR_MARK, "cursor-mark"),
    (DEVIN_MARK, "devin-mark"),
    (GROK_MARK, "grok-mark"),
    (HERMES_MARK, "hermes-mark"),
    (PI_MARK, "pi-mark"),
    (OPENCODE_MARK, "opencode-mark"),
    (ANTIGRAVITY_MARK, "antigravity-mark"),
];

pub const SETTINGS_GENERAL: &str = "icons/settings-general.svg";
pub const SETTINGS_PROVIDERS: &str = "icons/settings-providers.svg";
pub const SETTINGS_DEFAULTS: &str = "icons/settings-defaults.svg";
pub const SETTINGS_PLUGINS: &str = "icons/settings-plugins.svg";
pub const SETTINGS_REMOTE: &str = "icons/settings-remote.svg";

/// Serves both the compact control-icon set and the complete file-identity
/// icon theme through the single asset source registered at app startup.
pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if let Some(bytes) = zuse_icons::load_plugin_icon(path).or_else(|| zuse_icons::load(path)) {
            return Ok(Some(Cow::Borrowed(bytes)));
        }
        if let Some(asset) = ControlAssets.load(path)? {
            return Ok(Some(asset));
        }
        crate::file_icons::Assets.load(path)
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut assets = ControlAssets.list(path)?;
        assets.extend(crate::file_icons::Assets.list(path)?);
        assets.extend(
            zuse_icons::NAMES
                .iter()
                .filter(|name| name.starts_with(path))
                .map(|name| SharedString::from(*name)),
        );
        assets.sort();
        assets.dedup();
        Ok(assets)
    }
}

/// The Claude mark's brand orange (`#D97757`) — zuse keeps it even on the
/// monochrome surface.
pub fn claude_brand() -> Hsla {
    gpui::rgb(0xD97757).into()
}

/// An icon element for an embedded asset path. Size and colour are set by the
/// caller (`.size(..)`, `.text_color(..)`), matching the web app's
/// `[&_svg]:size-4` idiom.
/// GPUI's SVG painter reads the SVG's own text color; a color on its parent
/// does not provide the paint input. Always set `.text_color(..)` on the icon.
pub fn icon(path: &'static str) -> Svg {
    svg().path(path).flex_none()
}

/// The Zuse Icons sidebar glyph (icons.zuse.sh "Sidebar" / "Right
/// sidebar"): a rounded frame holding a panel whose width morphs 5.5 → 1.75
/// as the sidebar closes. gpui SVGs are static, so it is drawn from quads in
/// the source's 24-unit space, scaled to `size`. `open` is the morph progress
/// (1 = open; drive it with [`crate::motion::state_t`]); `right` mirrors it.
pub fn sidebar_glyph(open: f32, right: bool, size: f32, color: Hsla) -> Div {
    let s = size / 24.0;
    // `<rect x=3 y=4 w=18 h=16 rx=4 stroke-width=1.75>`: the centered stroke
    // grows the box by half the stroke on each side.
    let stroke = 1.75;
    let frame = div()
        .absolute()
        .left(px((3.0 - stroke / 2.0) * s))
        .top(px((4.0 - stroke / 2.0) * s))
        .w(px((18.0 + stroke) * s))
        .h(px((16.0 + stroke) * s))
        .rounded(px((4.0 + stroke / 2.0) * s))
        .border(px(stroke * s))
        .border_color(color);
    // `<rect class=zi-panel x=6.5 y=7.5 w=5.5 h=9 rx=.875>`; closed: w=1.75.
    let inset = px(6.5 * s);
    let panel = div()
        .absolute()
        .top(px(7.5 * s))
        .w(px(crate::motion::lerp(1.75, 5.5, open.clamp(0.0, 1.0)) * s))
        .h(px(9.0 * s))
        .rounded(px(0.875 * s))
        .bg(color);
    let panel = if right {
        panel.right(inset)
    } else {
        panel.left(inset)
    };
    div()
        .relative()
        .flex_none()
        .size(px(size))
        .child(frame)
        .child(panel)
}

/// The frame every panel toggle shares (`<rect x=3 y=4 w=18 h=16 rx=4
/// stroke-width=1.75>`), from quads so the strokes stay crisp at 16px.
fn panel_frame(s: f32, color: Hsla) -> Div {
    let stroke = 1.75;
    div()
        .absolute()
        .left(px((3.0 - stroke / 2.0) * s))
        .top(px((4.0 - stroke / 2.0) * s))
        .w(px((18.0 + stroke) * s))
        .h(px((16.0 + stroke) * s))
        .rounded(px((4.0 + stroke / 2.0) * s))
        .border(px(stroke * s))
        .border_color(color)
}

/// The session card toggle: [`sidebar_glyph`]'s frame with a panel in only
/// its top-right corner, drawn from the same quads so the strokes match the
/// sidebar toggles beside it.
pub fn corner_card_glyph(size: f32, color: Hsla) -> Div {
    let s = size / 24.0;
    let frame = panel_frame(s, color);
    // The open sidebar panel's footprint (x 12..17.5 from the right inset),
    // cut to its top half.
    let panel = div()
        .absolute()
        .top(px(7.5 * s))
        .right(px(6.5 * s))
        .w(px(5.5 * s))
        .h(px(4.5 * s))
        .rounded(px(0.875 * s))
        .bg(color);
    div()
        .relative()
        .flex_none()
        .size(px(size))
        .child(frame)
        .child(panel)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pull_request_icons_produce_visible_pixels() {
        let renderer = gpui::SvgRenderer::new(std::sync::Arc::new(Assets));
        for path in [
            PULL_REQUEST,
            DOCUMENT,
            FILE_CODE,
            FOLDER_WITH_FILES,
            CHECK,
            CHECKLIST,
            DANGER_TRIANGLE,
            CLOCK_CIRCLE,
            GLOBAL,
            COPY,
            REFRESH,
            ALT_ARROW_LEFT,
            ALT_ARROW_DOWN,
            ARROW_UP_RIGHT,
            EXPAND_ARROWS,
            COLLAPSE_ARROWS,
            CHAT_ROUND_LINE,
            GIT_BRANCH,
            CLOSE_CIRCLE,
            FOLDER_TREE,
        ] {
            let bytes = Assets.load(path).unwrap().unwrap();
            let image = renderer.render_single_frame(&bytes, 1.0).unwrap();
            assert!(
                image
                    .as_bytes(0)
                    .unwrap()
                    .chunks_exact(4)
                    .any(|pixel| pixel[3] != 0),
                "{path} rasterizes to an empty image"
            );
        }
    }

    #[test]
    fn every_registered_icon_loads_and_parses() {
        let assets = Assets;
        for path in assets.list("icons/").unwrap() {
            let bytes = assets
                .load(&path)
                .unwrap()
                .unwrap_or_else(|| panic!("missing asset {path}"));
            let text = std::str::from_utf8(&bytes).expect("icon svg is utf-8");
            assert!(text.contains("<svg"), "{path} is not an svg");
            assert!(text.contains("viewBox"), "{path} lacks a viewBox");
        }
    }

    #[test]
    fn unknown_paths_are_none() {
        assert!(Assets.load("icons/nope.svg").unwrap().is_none());
    }

    #[test]
    fn list_filters_by_prefix() {
        assert!(!Assets.list("icons/").unwrap().is_empty());
        assert!(Assets.list("fonts/").unwrap().is_empty());
    }
}
