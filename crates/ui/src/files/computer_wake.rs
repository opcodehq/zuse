//! A file tab on a cloud computer that is asleep. The tab opens at once with
//! a waking state, asks the chat's conversation to wake the computer (the
//! same wake a message's files use), and reads the file once the computer's
//! tools connect. A wake that fails, or never arrives, ends in an inline
//! error with a retry; closing the tab drops its wait.
use super::FilesSurface;
use crate::localization::message as tr;
use crate::theme::Theme;
use gpui::{AnyElement, App, ClickEvent, Context, SharedString, Task, Window, div, prelude::*, px};
use std::time::Duration;
use zuse_i18n::messages as msg;

/// How long a tab waits for a woken computer before offering a retry. A
/// computer normally starts well inside this; the bound keeps a lost wake
/// from leaving the tab waiting forever.
pub(super) const WAKE_TIMEOUT: Duration = Duration::from_secs(180);

#[derive(Default)]
pub(super) struct ComputerWake {
    /// This tab asked for the computer and is waiting for it.
    requested: bool,
    /// The computer came up for this tab; the file is still on its way.
    woke: bool,
    /// The wait outlasted [`WAKE_TIMEOUT`].
    timed_out: bool,
    timeout: Option<Task<()>>,
}

/// What a file tab shows in place of its file while the computer wakes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum WakeView {
    Waking,
    /// Woken; the file is being read.
    Loading,
    Failed(SharedString),
}

impl FilesSurface {
    /// An editor tab whose cloud computer must be woken before it can read
    /// its file. A tab already holding its document (perhaps with unsaved
    /// edits) keeps showing it while the computer sleeps.
    pub(super) fn awaiting_computer(&self, cx: &App) -> bool {
        self.presentation.is_editor()
            && !self.preview.has_active()
            && self.state.read(cx).cloud_computer_asleep()
    }

    /// Ask for the computer once per wait, and bound the wait.
    pub(super) fn request_computer_wake(&mut self, cx: &mut Context<Self>) {
        // A wait that already ended (the computer woke) starts afresh.
        if self.computer_wake.requested && !self.computer_wake.woke {
            return;
        }
        self.computer_wake = ComputerWake {
            requested: true,
            ..ComputerWake::default()
        };
        self.computer_wake.timeout = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(WAKE_TIMEOUT).await;
            let _ = this.update(cx, |this, cx| {
                if this.awaiting_computer(cx) || !this.preview.has_active() {
                    this.computer_wake.timed_out = true;
                    cx.notify();
                }
            });
        }));
        self.state
            .update(cx, |state, cx| state.request_cloud_wake(cx));
        cx.notify();
    }

    /// The state changed: once the computer is up, read the file.
    pub(super) fn observe_computer_wake(&mut self, cx: &mut Context<Self>) {
        if !self.computer_wake.requested {
            return;
        }
        if !self.awaiting_computer(cx) && !self.computer_wake.woke {
            self.computer_wake.woke = true;
            self.ensure_loaded(cx);
        }
        cx.notify();
    }

    /// Retry from the inline error: wake again, or read again if awake.
    pub(crate) fn retry_computer_wake(&mut self, cx: &mut Context<Self>) {
        self.computer_wake = ComputerWake::default();
        if self.awaiting_computer(cx) {
            self.request_computer_wake(cx);
        } else {
            self.computer_wake.requested = true;
            self.computer_wake.woke = true;
            self.ensure_loaded(cx);
        }
        cx.notify();
    }

    /// What stands in for the file while its computer wakes, if anything.
    pub(crate) fn wake_view(&self, cx: &App) -> Option<WakeView> {
        if !self.presentation.is_editor() {
            return None;
        }
        let awaiting = self.awaiting_computer(cx);
        let settling = self.computer_wake.woke && !self.preview.has_active();
        if !awaiting && !settling {
            return None;
        }
        if self.computer_wake.timed_out {
            return Some(WakeView::Failed(if awaiting {
                "The computer is taking too long to wake.".into()
            } else {
                "Couldn't open this file.".into()
            }));
        }
        if awaiting && let Some(failure) = self.state.read(cx).cloud_wake_failure.clone() {
            return Some(WakeView::Failed(failure));
        }
        Some(if awaiting {
            WakeView::Waking
        } else {
            WakeView::Loading
        })
    }

    pub(super) fn render_wake_view(
        &mut self,
        view: WakeView,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match view {
            WakeView::Waking => {
                loading_state("files-waking-computer", "Waking the computer…", theme)
            }
            WakeView::Loading => loading_state("files-waking-loading", "Loading file…", theme),
            WakeView::Failed(message) => retry_state(
                "files-wake-retry",
                message,
                tr(cx, msg::chat::attempts_retry),
                theme,
                cx.listener(|this, _, _, cx| this.retry_computer_wake(cx)),
            ),
        }
    }
}

#[cfg(test)]
impl FilesSurface {
    /// Show the tab's image as painting it would (a test draws no tab).
    pub(crate) fn show_active_image(&mut self, cx: &mut Context<Self>) {
        let image = self
            .preview
            .active
            .as_ref()
            .and_then(|path| self.preview.documents.get(path))
            .and_then(|document| document.image.clone());
        if let Some(image) = image {
            image.update(cx, |image, cx| image.activate(cx));
        }
    }

    /// The tab's image finished loading.
    pub(crate) fn active_image_loaded(&self, cx: &App) -> bool {
        self.preview
            .active
            .as_ref()
            .and_then(|path| self.preview.documents.get(path))
            .and_then(|document| document.image.as_ref())
            .is_some_and(|image| image.read(cx).is_loaded())
    }
}

/// A quiet centered loader with a one-line caption.
pub(super) fn loading_state(id: &'static str, caption: &'static str, theme: &Theme) -> AnyElement {
    div()
        .id(id)
        .debug_selector(move || id.to_owned())
        .flex_1()
        .size_full()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(10.0))
        .child(crate::loaders::retro_loader(id, 14.0, theme.loader_glyph()))
        .child(
            div()
                .text_size(px(12.0))
                .text_color(theme.text_muted)
                .child(caption),
        )
        .into_any_element()
}

/// A quiet centered error with a retry button.
pub(super) fn retry_state(
    id: &'static str,
    message: impl Into<SharedString>,
    label: impl Into<SharedString>,
    theme: &Theme,
    on_retry: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    let label: SharedString = label.into();
    div()
        .flex_1()
        .size_full()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(10.0))
        .px(px(28.0))
        .child(
            div()
                .text_center()
                .text_size(px(12.0))
                .text_color(theme.text_muted)
                .child(message.into()),
        )
        .child(
            crate::popover::btn_primary(theme, &label)
                .id(id)
                .debug_selector(move || id.to_owned())
                .cursor_pointer()
                .on_click(on_retry),
        )
        .into_any_element()
}
