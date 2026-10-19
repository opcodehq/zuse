//! Review and commit-diff tabs in the main area. They lead the conversation
//! tabs in the titlebar strip; while one is active it stands in for the
//! conversation, and closing it returns to the chat. The right pane keeps the
//! browser, terminal, files and history.
use super::*;

/// One chat panel's main-area tabs. Handles key [`Shell::diffs`].
#[derive(Debug, Default)]
pub(super) struct MainTabs {
    /// Strip order.
    tabs: Vec<u64>,
    /// The tab the main area shows; `None` shows the conversation.
    active: Option<u64>,
    /// The full changes review, when open. There is at most one.
    review: Option<u64>,
}

impl Shell {
    /// The main-area tab standing in for the conversation, if any.
    pub(super) fn active_main_diff(&self, cx: &App) -> Option<u64> {
        if !matches!(self.route, Route::Chat) || self.active_chat.is_empty() {
            return None;
        }
        self.main_tabs
            .get(&self.panel_key(cx))?
            .active
            .filter(|id| self.diffs.contains_key(id))
    }

    /// The open chat's main-area tabs in strip order: handle, title, and
    /// whether it is the review.
    pub(super) fn main_tab_rows(&self, cx: &App) -> Vec<(u64, SharedString, bool)> {
        if self.active_chat.is_empty() {
            return Vec::new();
        }
        let Some(tabs) = self.main_tabs.get(&self.panel_key(cx)) else {
            return Vec::new();
        };
        tabs.tabs
            .iter()
            .filter_map(|id| {
                let changes = self.diffs.get(id)?;
                let review = tabs.review == Some(*id);
                let title = if review {
                    SharedString::from("Review")
                } else {
                    changes.read(cx).tab_title()
                };
                Some((*id, title, review))
            })
            .collect()
    }

    /// Open (or focus) the review of every change in the chat's checkout.
    pub(super) fn open_review_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.active_chat.is_empty() {
            return;
        }
        let key = self.panel_key(cx);
        if let Some(id) = self
            .main_tabs
            .get(&key)
            .and_then(|tabs| tabs.review)
            .filter(|id| self.diffs.contains_key(id))
        {
            self.activate_main_tab(Some(id), cx);
            return;
        }
        let changes = cx.new(|cx| Changes::new(self.state.clone(), cx));
        let id = self.push_main_tab(changes, window, cx);
        self.main_tabs.entry(key).or_default().review = Some(id);
    }

    /// A History row click: the commit's own diff opens as a main-area tab.
    pub(super) fn open_commit_diff_tab(
        &mut self,
        commit: zuse_protocol::GitHistoryCommit,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.active_chat.is_empty() {
            return;
        }
        let changes = cx.new(|cx| Changes::for_commit(self.state.clone(), commit, cx));
        self.push_main_tab(changes, window, cx);
    }

    fn push_main_tab(
        &mut self,
        changes: Entity<Changes>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> u64 {
        let id = self.subscribe_diff(changes.clone(), window, cx);
        let tabs = self.main_tabs.entry(self.panel_key(cx)).or_default();
        tabs.tabs.push(id);
        tabs.active = Some(id);
        changes.update(cx, |changes, cx| changes.ensure_content(cx));
        cx.notify();
        id
    }

    /// Show a main-area tab, or the conversation for `None`.
    pub(super) fn activate_main_tab(&mut self, id: Option<u64>, cx: &mut Context<Self>) {
        let tabs = self.main_tabs.entry(self.panel_key(cx)).or_default();
        if tabs.active == id {
            return;
        }
        tabs.active = id;
        match id.and_then(|id| self.diffs.get(&id).cloned()) {
            Some(changes) => changes.update(cx, |changes, cx| changes.ensure_content(cx)),
            None => self.focus_composer(cx),
        }
        cx.notify();
    }

    /// Close a main-area tab. Closing the one on screen returns to the chat.
    pub(super) fn close_main_tab(&mut self, id: u64, cx: &mut Context<Self>) {
        let Some(tabs) = self.main_tabs.get_mut(&self.panel_key(cx)) else {
            return;
        };
        tabs.tabs.retain(|tab| *tab != id);
        if tabs.review == Some(id) {
            tabs.review = None;
        }
        let was_active = tabs.active == Some(id);
        if was_active {
            tabs.active = None;
        }
        self.diffs.remove(&id);
        self.diff_subs.remove(&id);
        if was_active {
            self.focus_composer(cx);
        }
        cx.notify();
    }

    /// The main column while a review or diff tab is active: the diff
    /// options row under the titlebar, then the diff.
    pub(super) fn render_main_diff(&mut self, id: u64, cx: &mut Context<Self>) -> AnyElement {
        let Some(changes) = self.diffs.get(&id).cloned() else {
            return Empty.into_any_element();
        };
        let theme = Theme::of(cx).clone();
        changes.update(cx, |changes, cx| changes.ensure_content(cx));
        let controls = changes.update(cx, |changes, cx| changes.render_header_controls(cx));
        div()
            .id("main-diff")
            .debug_selector(|| "main-diff".into())
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_col()
            .pt(px(Theme::TITLEBAR_HEIGHT))
            .child(crate::surface_chrome::toolbar(&theme).child(controls))
            .child(div().flex_1().min_h_0().child(changes))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{AppContext, TestAppContext};

    #[gpui::test]
    fn review_tab_replaces_the_chat_until_closed(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        cx.update(|cx| {
            gpui_base::init(cx);
            cx.set_global(Theme::default());
            crate::app_menus::init(cx);
        });
        let window = cx.add_window(|_, cx| {
            let state = cx.new(|_| AppState::new());
            Shell::new(
                state,
                EngineBootConfig {
                    target: None,
                    data_dir: dir.path().into(),
                    ipc_port: 0,
                    edge_url: "http://127.0.0.1:1".into(),
                    edge_token: None,
                    org_id: None,
                    workos_client_id: None,
                    default_harness: zuse_protocol::HarnessId::Mock,
                },
                cx,
            )
        });
        window
            .update(cx, |shell, window, cx| {
                // The new-session canvas has nothing to review.
                shell.open_review_tab(window, cx);
                assert!(shell.main_tab_rows(cx).is_empty());

                shell.active_chat = "chat".into();
                shell.open_review_tab(window, cx);
                let review = shell.active_main_diff(cx).expect("review is active");
                // Reopening focuses the same review; the right pane stays shut.
                shell.activate_main_tab(None, cx);
                assert_eq!(shell.active_main_diff(cx), None);
                shell.open_review_tab(window, cx);
                assert_eq!(shell.active_main_diff(cx), Some(review));
                assert_eq!(shell.main_tab_rows(cx).len(), 1);
                assert!(!shell.right_pane_open(cx));

                // Tabs belong to their chat.
                shell.active_chat = "other".into();
                assert_eq!(shell.active_main_diff(cx), None);
                shell.active_chat = "chat".into();
                assert_eq!(shell.active_main_diff(cx), Some(review));

                // Closing the active tab returns to the conversation.
                shell.close_main_tab(review, cx);
                assert_eq!(shell.active_main_diff(cx), None);
                assert!(shell.main_tab_rows(cx).is_empty());
                assert!(!shell.diffs.contains_key(&review));
            })
            .unwrap();
    }
}
