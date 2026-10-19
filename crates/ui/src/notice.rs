//! Shared notice chip — the tinted failure card the transcript renders for a
//! failed run (zuse chat-view.tsx `ErrorChip`). The composer's own notices
//! live in its attached tray (`composer/tray.rs`).

use gpui::{Div, FontWeight, SharedString, div, prelude::*, px};

use crate::theme::Theme;

/// The chip's header icon treatment, which also picks its metrics.
pub enum NoticeChipIcon {
    /// 20px accent-washed tile holding a 12px triangle; 10px inset/radius,
    /// fixed 12px text (the transcript's ErrorChip).
    Tile,
}

/// The failure notice (chat-view.tsx `ErrorChip`): a tinted rounded chip —
/// `border <accent>/[0.16]` over a `<accent>/[0.05]` wash, a subtle tinted
/// wash, never a bare stroke — with a header row (DangerTriangle tile +
/// medium label, a tiny copy button pinned to its top-right corner — failure
/// payloads are meant to be pasted, not screenshotted) and the message below.
/// The message WRAPS instead of truncating: failure payloads carry exit
/// statuses and stderr, and a one-line ellipsis was exactly what made
/// opcodehq/zuse#95 undiagnosable from the screenshot. `warning` picks the
/// amber palette (amber-400/amber-200) over the default red
/// (red-400/red-300). Callers chain their own chrome
/// (`w_full().overflow_hidden()`).
pub fn notice_chip(
    theme: &Theme,
    warning: bool,
    label: &'static str,
    message: impl Into<SharedString>,
    icon: NoticeChipIcon,
) -> Div {
    let (accent, muted) = if warning {
        (theme.warning, theme.warning_muted)
    } else {
        (theme.danger, theme.danger_muted)
    };
    let message = message.into();
    let copy_message = message.clone();
    let NoticeChipIcon::Tile = icon;
    let header_icon = div()
        .flex_none()
        .size(px(20.0))
        .rounded(px(6.0))
        .bg(accent.opacity(0.12))
        .flex()
        .items_center()
        .justify_center()
        .child(
            crate::icons::icon(crate::icons::DANGER_TRIANGLE)
                .size(px(12.0))
                .text_color(muted.opacity(0.8)),
        );
    div()
        .flex()
        .flex_col()
        .gap(px(6.0))
        .rounded(px(10.0))
        .border_1()
        .border_color(accent.opacity(0.16))
        .bg(accent.opacity(0.05))
        .px(px(10.0))
        .py(px(8.0))
        .text_size(px(12.0))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .child(header_icon)
                .child(
                    div()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(muted.opacity(0.8))
                        .child(label),
                )
                .child(div().flex_1())
                .child(
                    div()
                        .id("notice-copy")
                        .flex_none()
                        .size(px(20.0))
                        .rounded(px(6.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .hover(move |s| s.bg(accent.opacity(0.12)))
                        .on_click(move |_, _, cx| {
                            cx.stop_propagation();
                            cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                                copy_message.to_string(),
                            ));
                        })
                        .tooltip(crate::settings::widgets::text_tooltip("Copy message"))
                        .child(
                            crate::icons::icon(crate::icons::COPY)
                                .size(px(12.0))
                                .text_color(muted.opacity(0.8)),
                        ),
                ),
        )
        .child(
            div()
                .min_w_0()
                .w_full()
                .text_color(theme.text)
                .child(message),
        )
}
