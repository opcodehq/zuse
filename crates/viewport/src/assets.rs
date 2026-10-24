//! Native and browser controls load the same embedded vector assets.
use gpui::{AssetSource, SharedString, Styled, Svg, svg};
use std::borrow::Cow;

pub struct Assets;
macro_rules! assets {
    ($($name:literal),* $(,)?) => {
        impl AssetSource for Assets {
            fn load(&self, path: &str) -> gpui::Result<Option<Cow<'static, [u8]>>> {
                if let Some(bytes) = zuse_icons::load_file_icon(path).or_else(|| zuse_icons::load_plugin_icon(path).or_else(|| zuse_icons::load(path))) { return Ok(Some(Cow::Borrowed(bytes))); }
                Ok(match path {
                    $(concat!("icons/",$name,".svg") => Some(Cow::Borrowed(include_bytes!(concat!("../../ui/assets/icons/",$name,".svg")).as_slice())),)*
                    _ => None,
                })
            }
            fn list(&self, path: &str) -> gpui::Result<Vec<SharedString>> {
                let mut names: Vec<SharedString> = [$(concat!("icons/",$name,".svg")),*].into_iter().chain(zuse_icons::NAMES.iter().copied()).filter(|p|p.starts_with(path)).map(Into::into).collect();
                names.sort(); names.dedup(); Ok(names)
            }
        }
    }
}
assets!(
    "zuse-logo",
    "cloud-boxd",
    "site-github",
    "check-circle",
    "close-circle",
    "cloud-dither",
    "plus",
    "pen-new-square",
    "archive-minimalistic",
    "settings-minimalistic",
    "folder",
    "file-code",
    "git-branch",
    "terminal",
    "arrow-up",
    "arrow-right",
    "arrow-up-right",
    "alt-arrow-down",
    "alt-arrow-left",
    "alt-arrow-right",
    "arrow-left",
    "close",
    "paperclip",
    "claude-mark",
    "openai-mark",
    "cursor-mark",
    "opencode-mark",
    "pi-mark",
    "devin-mark",
    "grok-mark",
    "gemini-mark",
    "kiro-mark",
    "hermes-mark",
    "antigravity-mark",
    "check",
    "stop",
    "refresh",
    "split-columns",
    "queue-send",
    "queue-close",
    "queue-drag-handle",
    "file-tree",
    "globe",
    "clock-circle",
    "copy",
    "magnifer",
    "star",
    "danger-triangle",
    "trash-bin-minimalistic",
    "pen"
);

pub(super) fn icon(name: &str) -> Svg {
    svg()
        .path(SharedString::from(format!("icons/{name}.svg")))
        .text_color(crate::palette::rgb(0xa4a8a3))
        .size(gpui::px(16.))
        .flex_none()
}

pub(super) fn file_icon(path: &str, directory: bool) -> gpui::AnyElement {
    use gpui::prelude::*;
    if directory {
        icon("folder").size(gpui::px(14.)).into_any_element()
    } else {
        gpui::img(SharedString::from(zuse_icons::file_icon_path(path)))
            .size(gpui::px(14.))
            .flex_none()
            .into_any_element()
    }
}
