//! Review, commit-diff and file tabs in the main area. They lead the
//! conversation tabs in the titlebar strip; while one is active it stands in
//! for the conversation, and closing it returns to the chat. The right pane
//! keeps the file tree, browser, terminal and history.
use super::*;

/// One main-area tab: a diff (keys [`Shell::diffs`]) or a file editor (keys
/// [`Shell::file_surfaces`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum MainTab {
    Diff(u64),
    File(u64),
}

/// One chat panel's main-area tabs.
#[derive(Debug, Default)]
pub(super) struct MainTabs {
    /// Strip order.
    tabs: Vec<MainTab>,
    /// The tab the main area shows; `None` shows the conversation.
    active: Option<MainTab>,
    /// The full changes review, when open. There is at most one.
    review: Option<u64>,
}

/// A main-area tab as the titlebar strip draws it.
pub(super) struct MainTabRow {
    pub(super) tab: MainTab,
    pub(super) title: SharedString,
    /// The review of every change (rather than one commit's diff).
    pub(super) review: bool,
    /// A file with unsaved edits.
    pub(super) dirty: bool,
    /// A file's workspace path, for its icon and tooltip.
    pub(super) path: Option<SharedString>,
}

impl Shell {
    fn main_tab_live(&self, tab: MainTab) -> bool {
        match tab {
            MainTab::Diff(id) => self.diffs.contains_key(&id),
            MainTab::File(id) => self.file_surfaces.contains_key(&id),
        }
    }

    /// The main-area tab standing in for the conversation, if any.
    pub(super) fn active_main_tab(&self, cx: &App) -> Option<MainTab> {
        if !matches!(self.route, Route::Chat) || self.active_chat.is_empty() {
            return None;
        }
        self.main_tabs
            .get(&self.panel_key(cx))?
            .active
            .filter(|tab| self.main_tab_live(*tab))
    }

    /// The active main-area diff (review or commit), if one is on screen.
    pub(super) fn active_main_diff(&self, cx: &App) -> Option<u64> {
        match self.active_main_tab(cx)? {
            MainTab::Diff(id) => Some(id),
            MainTab::File(_) => None,
        }
    }

    /// The active main-area file editor, if one is on screen.
    pub(super) fn active_main_file(&self, cx: &App) -> Option<u64> {
        match self.active_main_tab(cx)? {
            MainTab::File(id) => Some(id),
            MainTab::Diff(_) => None,
        }
    }

    /// The open chat's main-area tabs in strip order.
    pub(super) fn main_tab_rows(&self, cx: &App) -> Vec<MainTabRow> {
        if self.active_chat.is_empty() {
            return Vec::new();
        }
        let Some(tabs) = self.main_tabs.get(&self.panel_key(cx)) else {
            return Vec::new();
        };
        tabs.tabs
            .iter()
            .filter_map(|tab| match *tab {
                MainTab::Diff(id) => {
                    let changes = self.diffs.get(&id)?;
                    let review = tabs.review == Some(id);
                    let title = if review {
                        SharedString::from("Review")
                    } else {
                        changes.read(cx).tab_title()
                    };
                    Some(MainTabRow {
                        tab: *tab,
                        title,
                        review,
                        dirty: false,
                        path: None,
                    })
                }
                MainTab::File(id) => {
                    let file = self.file_surfaces.get(&id)?;
                    let path = self.file_surface_paths.get(&id);
                    Some(MainTabRow {
                        tab: *tab,
                        title: path
                            .map(|path| workspace_file_title(path))
                            .unwrap_or_else(|| SharedString::from("File")),
                        review: false,
                        dirty: file.read(cx).has_unsaved_changes(),
                        path: path.cloned().map(Into::into),
                    })
                }
            })
            .collect()
    }

    /// Open (or focus) the review of every change in the chat's checkout.
    pub(super) fn open_review_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.active_chat.is_empty() {
            return;
        }
        let key = self.panel_key(cx);
        self.collapse_surfaces_if_empty(&key, cx);
        if let Some(id) = self
            .main_tabs
            .get(&key)
            .and_then(|tabs| tabs.review)
            .filter(|id| self.diffs.contains_key(id))
        {
            self.activate_main_tab(Some(MainTab::Diff(id)), cx);
            return;
        }
        let changes = cx.new(|cx| Changes::new(self.conversation_state(cx), cx));
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
        self.suspend_main_file_images(cx);
        let tabs = self.main_tabs.entry(self.panel_key(cx)).or_default();
        tabs.tabs.push(MainTab::Diff(id));
        tabs.active = Some(MainTab::Diff(id));
        changes.update(cx, |changes, cx| {
            changes.review_mode = true;
            changes.ensure_content(cx);
        });
        cx.notify();
        id
    }

    /// Add a file editor's tab to `panel_key`'s strip (after its other tabs)
    /// and show it.
    pub(super) fn push_main_file_tab(&mut self, panel_key: &str, id: u64, cx: &mut Context<Self>) {
        let tabs = self.main_tabs.entry(panel_key.to_owned()).or_default();
        if !tabs.tabs.contains(&MainTab::File(id)) {
            tabs.tabs.push(MainTab::File(id));
        }
        self.activate_main_tab(Some(MainTab::File(id)), cx);
    }

    /// Show `id`'s file tab in `panel_key`'s strip without navigating there
    /// (an unsaved editor surfacing for a quit prompt).
    pub(super) fn select_main_file_tab(&mut self, panel_key: &str, id: u64) {
        if let Some(tabs) = self.main_tabs.get_mut(panel_key)
            && tabs.tabs.contains(&MainTab::File(id))
        {
            tabs.active = Some(MainTab::File(id));
        }
    }

    /// Release image previews of file tabs that are not on screen.
    fn suspend_main_file_images(&mut self, cx: &mut Context<Self>) {
        if let Some(id) = self.active_main_file(cx)
            && let Some(file) = self.file_surfaces.get(&id).cloned()
        {
            file.update(cx, |file, cx| file.suspend_images(cx));
        }
    }

    /// Show a main-area tab, or the conversation for `None`.
    pub(super) fn activate_main_tab(&mut self, tab: Option<MainTab>, cx: &mut Context<Self>) {
        let current = self
            .main_tabs
            .get(&self.panel_key(cx))
            .and_then(|tabs| tabs.active);
        if current == tab {
            return;
        }
        self.suspend_main_file_images(cx);
        self.main_tabs.entry(self.panel_key(cx)).or_default().active = tab;
        match tab {
            Some(MainTab::Diff(id)) => {
                if let Some(changes) = self.diffs.get(&id).cloned() {
                    changes.update(cx, |changes, cx| changes.ensure_content(cx));
                }
            }
            Some(MainTab::File(id)) => {
                if let Some(file) = self.file_surfaces.get(&id).cloned() {
                    file.update(cx, |file, cx| file.ensure_loaded(cx));
                    if let Some(path) = self.file_surface_paths.get(&id).cloned() {
                        let chat_id = file.read(cx).chat_id().to_owned();
                        self.note_recent_file(&chat_id, &path);
                    }
                }
                self.composer
                    .update(cx, |composer, _| composer.focus_pending = false);
                self.sync_explorer_selection(cx);
            }
            None => self.focus_composer(cx),
        }
        cx.notify();
    }

    /// [`Self::activate_main_tab`], then focus a file tab's editor.
    pub(super) fn focus_main_tab(
        &mut self,
        tab: Option<MainTab>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.activate_main_tab(tab, cx);
        if let Some(MainTab::File(id)) = tab
            && let Some(file) = self.file_surfaces.get(&id).cloned()
        {
            file.update(cx, |file, cx| file.focus_editor(window, cx));
        }
    }

    /// Close a main-area tab. Closing the one on screen returns to the chat.
    /// A file with unsaved edits asks first (its tab stays until it may close).
    pub(super) fn close_main_tab(&mut self, tab: MainTab, cx: &mut Context<Self>) {
        let key = self.panel_key(cx);
        if let MainTab::File(id) = tab {
            let Some(file) = self.file_surfaces.get(&id).cloned() else {
                return;
            };
            match file.update(cx, |file, cx| file.prepare_close(cx)) {
                FilesCloseDisposition::Allow => {
                    self.complete_file_close(RightSurface::File(id), &key, cx)
                }
                FilesCloseDisposition::Pending | FilesCloseDisposition::Blocked => {
                    self.pending_file_closes.insert(RightSurface::File(id));
                    self.activate_main_tab(Some(tab), cx);
                }
            }
            return;
        }
        let Some(tabs) = self.main_tabs.get_mut(&key) else {
            return;
        };
        let MainTab::Diff(id) = tab else {
            return;
        };
        tabs.tabs.retain(|candidate| *candidate != tab);
        if tabs.review == Some(id) {
            tabs.review = None;
        }
        let was_active = tabs.active == Some(tab);
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

    /// Drop a closed file editor from every strip; one on screen hands the
    /// main area back to the conversation.
    pub(super) fn forget_main_file_tab(&mut self, id: u64, cx: &mut Context<Self>) {
        let current = self.panel_key(cx);
        let mut refocus = false;
        for (key, tabs) in self.main_tabs.iter_mut() {
            tabs.tabs.retain(|tab| *tab != MainTab::File(id));
            if tabs.active == Some(MainTab::File(id)) {
                tabs.active = None;
                refocus |= *key == current;
            }
        }
        if refocus {
            self.focus_composer(cx);
        }
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
            .child(
                crate::surface_chrome::toolbar(&theme)
                    .h(px(44.))
                    .px(px(16.))
                    .border_0()
                    .bg(theme.bg)
                    .child(controls),
            )
            .child(div().flex_1().min_h_0().child(changes))
            .into_any_element()
    }

    /// The main column while a file tab is active: the editor (its own
    /// breadcrumb toolbar on top) under the titlebar.
    pub(super) fn render_main_file(&mut self, id: u64, cx: &mut Context<Self>) -> AnyElement {
        let Some(file) = self.file_surfaces.get(&id).cloned() else {
            return Empty.into_any_element();
        };
        file.update(cx, |file, cx| file.ensure_loaded(cx));
        div()
            .id("main-file")
            .debug_selector(|| "main-file".into())
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_col()
            .pt(px(Theme::TITLEBAR_HEIGHT))
            .child(div().flex_1().min_h_0().child(file))
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
                shell.close_main_tab(MainTab::Diff(review), cx);
                assert_eq!(shell.active_main_diff(cx), None);
                assert!(shell.main_tab_rows(cx).is_empty());
                assert!(!shell.diffs.contains_key(&review));
            })
            .unwrap();
    }
}
