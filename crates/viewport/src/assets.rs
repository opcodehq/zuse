//! Native and browser controls load the same embedded vector assets.
use gpui::{AssetSource, SharedString, Styled, Svg, svg};
use std::borrow::Cow;

pub struct Assets;
macro_rules! assets {
    ($($name:literal),* $(,)?) => {
        impl AssetSource for Assets {
            fn load(&self, path: &str) -> gpui::Result<Option<Cow<'static, [u8]>>> {
                Ok(match path {
                    $(concat!("icons/",$name,".svg") => Some(Cow::Borrowed(include_bytes!(concat!("../../ui/assets/icons/",$name,".svg")).as_slice())),)*
                    _ => None,
                })
            }
            fn list(&self, path: &str) -> gpui::Result<Vec<SharedString>> {
                Ok([$(concat!("icons/",$name,".svg")),*].into_iter().filter(|p|p.starts_with(path)).map(Into::into).collect())
            }
        }
    }
}
assets!(
    "zuse-logo",
    "plus",
    "pen-new-square",
    "archive-minimalistic",
    "settings-minimalistic",
    "folder",
    "file-code",
    "git-branch",
    "terminal",
    "arrow-up",
    "alt-arrow-down",
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
    "magnifer"
);

pub(super) fn icon(name: &str) -> Svg {
    svg()
        .path(SharedString::from(format!("icons/{name}.svg")))
        .text_color(gpui::rgb(0x929292))
        .size(gpui::px(16.))
        .flex_none()
}
