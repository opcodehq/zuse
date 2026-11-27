//! A file tab on a cloud computer whose tools are not connected. The tab
//! opens at once and asks the chat's conversation for the computer: a
//! sleeping computer is woken (the same wake a message's files use), a
//! running one just has its connection opened. The file is read as soon as
//! the connection is there, whatever brought it up. A request that fails,
//! or is never answered, ends in an inline error with a retry within
//! [`WAKE_TIMEOUT`]; closing the tab drops its wait.
use super::FilesSurface;
use crate::localization::message as tr;
use crate::theme::Theme;
use gpui::{AnyElement, App, ClickEvent, Context, SharedString, Task, Window, div, prelude::*, px};
use std::time::Duration;
use zuse_i18n::messages as msg;

/// How long a tab waits for the computer before offering a retry. A
/// computer normally starts well inside this; the bound keeps a lost wake
/// from leaving the tab waiting forever.
pub(super) const WAKE_TIMEOUT: Duration = Duration::from_secs(180);

/// One wait for the computer, from the first request until the file shows,
/// the wait fails, or Retry starts a new one.
#[derive(Default)]
pub(super) struct ComputerWake {
    /// This tab asked for the computer and has not shown its file yet.
    requested: bool,
    /// The wait outlasted [`WAKE_TIMEOUT`].
    timed_out: bool,
    /// Ends the wait at [`WAKE_TIMEOUT`]. Set once per wait: asking again
    /// (a connection that dropped and is reopening) never extends it.
    timeout: Option<Task<()>>,
}

/// What a file tab shows in place of its file while it waits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum WakeView {
    /// The computer is asleep and is being woken.
    Waking,
    /// The computer is up; the file is being read (or its connection
    /// opened first).
    Loading,
    Failed(SharedString),
}

impl FilesSurface {
    /// An editor tab whose cloud computer's tools are not connected, so it
    /// cannot read its file yet. A tab already holding its document
    /// (perhaps with unsaved edits) keeps showing it meanwhile.
    pub(super) fn awaiting_computer(&self, cx: &App) -> bool {
        self.presentation.is_editor()
            && !self.preview.has_active()
            && self.state.read(cx).cloud_computer_asleep()
    }

    /// Ask for the computer once per wait, and bound the wait.
    pub(super) fn request_computer_wake(&mut self, cx: &mut Context<Self>) {
        // One request per wait; a wait that ran out waits for Retry.
        if self.computer_wake.requested || self.computer_wake.timed_out {
            return;
        }
        self.start_computer_wait(cx);
        self.state
            .update(cx, |state, cx| state.request_cloud_wake(cx));
        cx.notify();
    }

    /// Start the wait's deadline. Whatever happens on the computer's side,
    /// the tab ends in its file or an error with Retry by then.
    fn start_computer_wait(&mut self, cx: &mut Context<Self>) {
        self.computer_wake.requested = true;
        self.computer_wake.timeout = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(WAKE_TIMEOUT).await;
            let _ = this.update(cx, |this, cx| this.computer_wait_expired(cx));
        }));
    }

    fn computer_wait_expired(&mut self, cx: &mut Context<Self>) {
        self.computer_wake.timeout = None;
        if self.preview.has_active() {
            self.computer_wake = ComputerWake::default();
            return;
        }
        self.computer_wake.requested = false;
        self.computer_wake.timed_out = true;
        cx.notify();
    }

    /// The state changed: as soon as the computer's tools are connected,
    /// read the file. This follows the connection itself, so a computer
    /// that was already up (no wake to report) is read the same way.
    pub(super) fn observe_computer_wake(&mut self, cx: &mut Context<Self>) {
        if !self.computer_wake.requested {
            return;
        }
        if !self.preview.has_active() && !self.awaiting_computer(cx) {
            self.ensure_loaded(cx);
        }
        if self.preview.has_active() {
            // The file is open; its own view takes it from here.
            self.computer_wake = ComputerWake::default();
        }
        cx.notify();
    }

    /// Retry from the inline error: start a new wait, or read again if the
    /// computer is already connected.
    pub(crate) fn retry_computer_wake(&mut self, cx: &mut Context<Self>) {
        self.computer_wake = ComputerWake::default();
        if self.awaiting_computer(cx) {
            self.request_computer_wake(cx);
        } else {
            self.ensure_loaded(cx);
            if !self.preview.has_active() {
                // Connected but not readable yet (the chat's files are
                // still arriving): this wait is bounded too.
                self.start_computer_wait(cx);
            }
        }
        cx.notify();
    }

    /// What stands in for the file while it waits on the computer, if
    /// anything.
    pub(crate) fn wake_view(&self, cx: &App) -> Option<WakeView> {
        if !self.presentation.is_editor() || self.preview.has_active() {
            return None;
        }
        let awaiting = self.awaiting_computer(cx);
        let waiting = self.computer_wake.requested || self.computer_wake.timed_out;
        if !awaiting && !waiting {
            return None;
        }
        let state = self.state.read(cx);
        let online = state.cloud_computer_online;
        if self.computer_wake.timed_out {
            return Some(WakeView::Failed(if awaiting && !online {
                "The computer is taking too long to wake.".into()
            } else if awaiting {
                "Couldn't reach the computer.".into()
            } else {
                "Couldn't open this file.".into()
            }));
        }
        if awaiting && let Some(failure) = state.cloud_wake_failure.clone() {
            return Some(WakeView::Failed(failure));
        }
        // A computer that is up is never shown as waking.
        Some(if awaiting && !online {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{AppState, EngineHandle};
    use gpui::{Entity, TestAppContext};
    use std::sync::{Arc, Mutex};
    use zuse_protocol::methods;

    /// A connected computer's files: records each read and never answers,
    /// which is all a tab needs to have started reading.
    #[derive(Default)]
    struct Reads(Mutex<Vec<String>>);
    #[async_trait::async_trait]
    impl zuse_rpc::RpcService for Reads {
        async fn handle(
            &self,
            method: &str,
            _: serde_json::Value,
        ) -> Result<zuse_rpc::RpcReply, zuse_rpc::RpcError> {
            self.0.lock().unwrap().push(method.to_owned());
            Ok(zuse_rpc::RpcReply::Stream(Box::pin(
                futures::stream::pending(),
            )))
        }
    }

    /// A cloud chat's state with no tools connected.
    fn cloud_state(online: bool, cx: &mut TestAppContext) -> Entity<AppState> {
        cx.new(|_| {
            let mut state = AppState::new();
            state.hosted_on_cloud_computer = true;
            state.cloud_computer_online = online;
            state.local_device_id = Some("cloud-computer".into());
            state.apply_chats(vec![
                serde_json::from_value(serde_json::json!({
                    "id": "session", "deviceId": "cloud-computer", "archived": false,
                    "cwd": "/home/zuse/workspace", "spaceId": "space",
                    "checkoutId": "checkout", "createdAt": "2026-11-27T00:00:00Z",
                }))
                .unwrap(),
            ]);
            state
        })
    }

    fn editor(state: &Entity<AppState>, cx: &mut TestAppContext) -> Entity<FilesSurface> {
        let state = state.clone();
        let file = cx.new(|cx| {
            FilesSurface::new_editor(
                state,
                "session".into(),
                "/home/zuse/workspace/notes.txt".into(),
                false,
                1000,
                13.0,
                false,
                false,
                cx,
            )
        });
        file.update(cx, |file, cx| file.ensure_loaded(cx));
        cx.run_until_parked();
        file
    }

    fn connect(state: &Entity<AppState>, cx: &mut TestAppContext) -> Arc<Reads> {
        let reads = Arc::new(Reads::default());
        let engine = EngineHandle::from_test_client(zuse_rpc::memory_client(reads.clone()));
        state.update(cx, |state, cx| {
            state.set_test_engine(engine);
            cx.notify();
        });
        cx.run_until_parked();
        reads
    }

    fn init(cx: &mut TestAppContext) -> tokio::runtime::Runtime {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .unwrap();
        cx.executor().allow_parking();
        cx.update(|cx| {
            gpui_base::init(cx);
            cx.set_global(Theme::default());
        });
        runtime
    }

    fn requests(state: &Entity<AppState>, cx: &mut TestAppContext) -> u64 {
        state.read_with(cx, |state, _| state.cloud_wake_requests)
    }

    /// A computer that is up is never shown waking: the tab says it is
    /// loading while the connection opens, and reads the file the moment
    /// the connection is there (no wake event to wait for).
    #[gpui::test]
    fn an_online_computers_file_loads_without_a_waking_state(cx: &mut TestAppContext) {
        let runtime = init(cx);
        let _entered = runtime.enter();
        let state = cloud_state(true, cx);
        let file = editor(&state, cx);
        file.read_with(cx, |file, cx| {
            assert_eq!(file.wake_view(cx), Some(WakeView::Loading))
        });
        assert_eq!(requests(&state, cx), 1, "the connection is asked for");

        let reads = connect(&state, cx);
        file.read_with(cx, |file, cx| {
            assert_eq!(file.wake_view(cx), None);
            assert!(file.preview.has_active(), "the file is open");
        });
        for _ in 0..200 {
            if !reads.0.lock().unwrap().is_empty() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
            cx.run_until_parked();
        }
        assert!(
            reads
                .0
                .lock()
                .unwrap()
                .iter()
                .any(|method| method == methods::READ_WORKSPACE_FILE),
            "the file is read from the computer"
        );
        assert_eq!(requests(&state, cx), 1, "nothing else is asked for");
    }

    /// A sleeping computer shows the waking state until its tools connect,
    /// then the file.
    #[gpui::test]
    fn a_sleeping_computers_file_wakes_then_loads(cx: &mut TestAppContext) {
        let runtime = init(cx);
        let _entered = runtime.enter();
        let state = cloud_state(false, cx);
        let file = editor(&state, cx);
        file.read_with(cx, |file, cx| {
            assert_eq!(file.wake_view(cx), Some(WakeView::Waking))
        });
        assert_eq!(requests(&state, cx), 1);
        // The computer reports itself up before its tools connect.
        state.update(cx, |state, cx| {
            state.cloud_computer_online = true;
            cx.notify();
        });
        cx.run_until_parked();
        file.read_with(cx, |file, cx| {
            assert_eq!(file.wake_view(cx), Some(WakeView::Loading))
        });
        connect(&state, cx);
        file.read_with(cx, |file, cx| {
            assert_eq!(file.wake_view(cx), None);
            assert!(file.preview.has_active());
        });
        assert_eq!(requests(&state, cx), 1);
    }

    /// A wake that never completes ends in an error with Retry once the cap
    /// runs out, asking again in the meantime never extends it, and Retry
    /// starts a new, equally bounded wait.
    #[gpui::test]
    fn a_wake_that_never_completes_offers_retry_within_the_cap(cx: &mut TestAppContext) {
        let runtime = init(cx);
        let _entered = runtime.enter();
        for online in [false, true] {
            let state = cloud_state(online, cx);
            let file = editor(&state, cx);
            cx.executor().advance_clock(WAKE_TIMEOUT / 2);
            // Asking again mid-wait (the tab refocused) changes nothing.
            file.update(cx, |file, cx| file.ensure_loaded(cx));
            cx.run_until_parked();
            assert_eq!(requests(&state, cx), 1);
            cx.executor().advance_clock(WAKE_TIMEOUT / 2);
            cx.run_until_parked();
            let failed = file.read_with(cx, |file, cx| file.wake_view(cx));
            assert_eq!(
                failed,
                Some(WakeView::Failed(if online {
                    "Couldn't reach the computer.".into()
                } else {
                    "The computer is taking too long to wake.".into()
                })),
            );
            // Refocusing a failed tab keeps the error until Retry.
            file.update(cx, |file, cx| file.ensure_loaded(cx));
            assert_eq!(requests(&state, cx), 1);

            file.update(cx, |file, cx| file.retry_computer_wake(cx));
            cx.run_until_parked();
            assert_eq!(requests(&state, cx), 2, "Retry asks again");
            file.read_with(cx, |file, cx| {
                assert_eq!(
                    file.wake_view(cx),
                    Some(if online {
                        WakeView::Loading
                    } else {
                        WakeView::Waking
                    })
                )
            });
            cx.executor().advance_clock(WAKE_TIMEOUT);
            cx.run_until_parked();
            file.read_with(cx, |file, cx| {
                assert!(matches!(file.wake_view(cx), Some(WakeView::Failed(_))))
            });
        }
    }

    /// A tab left on a cloud chat's old state (its conversation closed and
    /// reopened while the chat was not in front) follows the new state and
    /// reads through it.
    #[gpui::test]
    fn a_tab_rebound_to_a_reopened_chat_reads_through_it(cx: &mut TestAppContext) {
        let runtime = init(cx);
        let _entered = runtime.enter();
        let old = cloud_state(false, cx);
        let file = editor(&old, cx);
        file.read_with(cx, |file, cx| {
            assert_eq!(file.wake_view(cx), Some(WakeView::Waking))
        });
        let reopened = cloud_state(true, cx);
        connect(&reopened, cx);
        file.update(cx, |file, cx| file.rebind_state(reopened.clone(), cx));
        cx.run_until_parked();
        file.read_with(cx, |file, cx| {
            assert_eq!(file.state(), &reopened);
            assert_eq!(file.wake_view(cx), None);
            assert!(file.preview.has_active());
        });
    }
}
