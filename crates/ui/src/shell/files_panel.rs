//! The session-owned file tree, shown as the right pane's Files tab. Files
//! it opens become main-area tabs (see `main_tabs`).
//!
//! The width plumbing below predates the tab: the tree used to dock as its
//! own column. That column no longer opens (`ChatPanels::files_open` stays
//! false), so every width here resolves to zero.

use super::*;

/// Files kept per chat for the command palette's Recent Files.
const RECENT_FILES_PER_CHAT: usize = 20;

impl Shell {
    /// Record `path` as the chat's most recently opened file.
    pub(super) fn note_recent_file(&mut self, chat_id: &str, path: &str) {
        if chat_id.is_empty() {
            return;
        }
        let recent = self.recent_files.entry(chat_id.to_owned()).or_default();
        recent.retain(|candidate| candidate != path);
        recent.push_front(path.to_owned());
        recent.truncate(RECENT_FILES_PER_CHAT);
    }

    /// The chat's opened files, most recent first.
    pub(super) fn recent_files(&self, chat_id: &str) -> impl Iterator<Item = &String> {
        self.recent_files.get(chat_id).into_iter().flatten()
    }
}

/// Allocate a real column to Files, reducing its preferred width before the
/// chat or surface minima. The shell fit policy hides a whole column when its
/// minimum cannot fit, so no settled column is rendered as a sliver.
#[derive(Debug, Clone, Copy, PartialEq)]
struct FilesPanelLayout {
    width: f32,
    surface_max: f32,
}

/// `surface_reveal` is how far the surface host is open, from 0 to 1. Files
/// yields the host's minimum in step with its open/close tween rather than
/// all at once when the flag flips.
fn files_panel_layout(
    viewport: f32,
    sidebar: f32,
    preferred: f32,
    visible: f32,
    surface_reveal: f32,
    expanded: bool,
) -> FilesPanelLayout {
    let surfaces_open = surface_reveal > 0.0;
    let available = (viewport - sidebar).max(0.0);
    // Takeover gives the conversation's space to the surface host. It must
    // not also widen Files: that would move the seam before the right tween
    // has painted its first frame, and move it back again on reversal.
    let surface_chat_min = if surfaces_open && expanded {
        0.0
    } else {
        CHAT_PANEL_MIN
    };
    let surface_min = RIGHT_PANE_MIN * surface_reveal.clamp(0.0, 1.0);
    let max_width = if preferred > 0.0 {
        (available - CHAT_PANEL_MIN - surface_min).max(0.0)
    } else {
        0.0
    };
    let width = visible.max(0.0).min(max_width);
    FilesPanelLayout {
        width,
        surface_max: right_pane_max_width(viewport - width, sidebar, surface_chat_min),
    }
}

impl Shell {
    pub(super) fn files_column_open(&self, cx: &App) -> bool {
        matches!(self.route, Route::Chat)
            && !self.active_chat.is_empty()
            && self.panels.get(&self.panel_key(cx)).files_open
    }

    fn files_layout(&self, visible: f32, cx: &App) -> FilesPanelLayout {
        let fit = self.horizontal_fit();
        files_panel_layout(
            self.viewport_width,
            self.sidebar_now(),
            if fit.files && (self.files_column_open(cx) || self.tween_active(self.files_tween)) {
                self.settings.files_panel_width
            } else {
                0.0
            },
            if fit.files { visible } else { 0.0 },
            if fit.right {
                self.right_reveal()
            } else {
                // A host the fit is hiding still occupies its exit width.
                self.fit_exit_width(1)
                    .map_or(0.0, |width| (width / RIGHT_PANE_MIN).clamp(0.0, 1.0))
            },
            self.right_pane_expanded && fit.right,
        )
    }

    /// How far the surface host is open: its open/close tween's progress,
    /// 1 at rest while open. Width-only tweens between open sizes stay at 1.
    fn right_reveal(&self) -> f32 {
        // What the host actually occupies, up to its minimum: continuous
        // through reversals, which restart the tween from a partial width.
        match self
            .right_tween
            .filter(|tween| self.tween_active(Some(*tween)))
        {
            Some(tween) => {
                (self.eval_tween(Some(tween), tween.to) / RIGHT_PANE_MIN).clamp(0.0, 1.0)
            }
            None => 1.0,
        }
    }

    pub(super) fn files_target(&self, cx: &App) -> f32 {
        self.files_layout(
            if self.files_column_open(cx) {
                self.settings.files_panel_width
            } else {
                0.0
            },
            cx,
        )
        .width
    }

    pub(super) fn files_visible_width(&self, cx: &App) -> f32 {
        if !matches!(self.route, Route::Chat) || self.active_chat.is_empty() {
            return 0.0;
        }
        if !self.horizontal_fit().files
            && let Some(exiting) = self.fit_exit_width(2)
        {
            // Exits clamp in layout order so the columns never overflow.
            return exiting.min((self.viewport_width - self.sidebar_now()).max(0.0));
        }
        self.files_layout(
            self.ease_toward(self.files_tween, self.files_target(cx)),
            cx,
        )
        .width
    }

    pub(super) fn files_reserved_width(&self, cx: &App) -> f32 {
        self.files_visible_width(cx)
    }

    /// Files' width once every running open/close has landed. Initial surface
    /// targets use this allocation; painted widths follow the live layout.
    pub(super) fn files_settled_width(&self, cx: &App) -> f32 {
        let (fit, sidebar) = self.settled_fit();
        self.files_width_for(fit, sidebar, cx)
    }

    /// Files' width for a given fit and sidebar width: its own width, as far
    /// as the conversation and the surface host's minimums leave room.
    pub(super) fn files_width_for(&self, fit: HorizontalPanelFit, sidebar: f32, cx: &App) -> f32 {
        let preferred = if fit.files && self.files_column_open(cx) {
            self.settings.files_panel_width
        } else {
            0.0
        };
        let right = fit.right && self.right_pane_open(cx);
        files_panel_layout(
            self.viewport_width,
            sidebar,
            preferred,
            preferred,
            if right { 1.0 } else { 0.0 },
            self.right_pane_expanded && right,
        )
        .width
    }

    pub(super) fn surface_max_width(&self, cx: &App) -> f32 {
        self.files_layout(self.files_visible_width(cx), cx)
            .surface_max
    }

    pub(super) fn right_visible_width(&self, cx: &App) -> f32 {
        if !self.horizontal_fit().right {
            let available =
                (self.viewport_width - self.sidebar_now() - self.files_visible_width(cx)).max(0.0);
            return self
                .fit_exit_width(1)
                .map_or(0.0, |width| width.min(available));
        }
        let available =
            (self.viewport_width - self.sidebar_now() - self.files_visible_width(cx)).max(0.0);
        // Leaving takeover returns the conversation's width over the same
        // tween. Applying its settled minimum here jumps the seam by 300px
        // before that first frame (and breaks mid-flight reversals).
        let max = if self.tween_active(self.main_takeover_tween) {
            available
        } else {
            self.surface_max_width(cx)
        };
        self.right_now(max, cx).min(available)
    }

    pub(super) fn files_content_width(&self, cx: &App) -> f32 {
        self.column_content_width(
            (2, self.horizontal_fit().files),
            self.files_tween,
            self.files_content_tween,
            self.files_target(cx),
        )
    }

    /// Ease Files from what it painted (`from`, its content laid out at
    /// `content`) to `to`, both read before the action changed anything.
    pub(super) fn transition_files(&mut self, from: f32, content: f32, to: f32) {
        let (content_from, content_to) = panel_content_tween(content, from, to);
        let mask = self.panel_tween(from, to);
        self.files_tween = Some(mask);
        self.files_content_tween = Some(WidthTween {
            from: content_from,
            to: content_to,
            ..mask
        });
    }

    pub(super) fn accepts_file_navigation(
        &self,
        owner: &str,
        source: &Entity<FilesSurface>,
        cx: &App,
    ) -> bool {
        matches!(self.route, Route::Chat)
            && self.panel_key(cx) == owner
            && source.read(cx).is_current_target(cx)
    }

    pub(super) fn prune_file_explorers(&mut self, cx: &mut Context<Self>) {
        let state = self.state.read(cx);
        if !state.chats_synced {
            return;
        }
        let cloud_session = self
            .cloud_conversation
            .as_ref()
            .map(|view| view.read(cx).target.initial_session.clone());
        let live = state
            .chats
            .iter()
            .map(|chat| chat.id.as_str())
            .chain(cloud_session.as_deref())
            .collect::<std::collections::HashSet<_>>();
        for key in self.files.keys().filter(|key| !live.contains(key.as_str())) {
            self.panels.update(key, |panels| panels.files_open = false);
        }
        self.files.retain(|key, _| live.contains(key.as_str()));
        self.files_subs.retain(|key, _| live.contains(key.as_str()));
    }

    /// Keep the tree's selection on the file tab the main area shows.
    pub(super) fn sync_explorer_selection(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.active_main_file(cx) else {
            return;
        };
        let Some(path) = self.file_surface_paths.get(&id).cloned() else {
            return;
        };
        if let Some(files) = self.files.get(&self.panel_key(cx)).cloned() {
            files.update(cx, |files, cx| files.reveal_file(path, cx));
        }
    }

    /// Whether the right pane shows the Files tab.
    pub(super) fn files_tab_shown(&self, cx: &App) -> bool {
        self.right_pane_open(cx) && self.resolved_right_active(cx) == RightSurface::Files
    }

    pub(super) fn add_files_surface(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self
            .cloud_conversation
            .as_ref()
            .is_some_and(|view| !view.read(cx).ready(cx))
        {
            return;
        }
        if self.active_chat.is_empty() {
            return;
        }
        let key = self.panel_key(cx);
        if !self.files.contains_key(&key) {
            let files = cx.new(|cx| {
                FilesSurface::new_explorer(
                    self.conversation_state(cx),
                    self.active_chat.clone(),
                    self.settings.files_show_all,
                    cx,
                )
            });
            let owner = key.clone();
            let sub = cx.subscribe_in(
                &files,
                window,
                move |this: &mut Self, source, event, window, cx| match event {
                    FilesEvent::HoldMutation { origin, paths } => {
                        let surfaces = this
                            .files
                            .values()
                            .chain(this.file_surfaces.values())
                            .filter(|s| s.read(cx).shares_workspace(origin))
                            .cloned()
                            .collect::<Vec<_>>();
                        for surface in surfaces {
                            surface.update(cx, |files, cx| files.hold_mutation(paths.clone(), cx));
                        }
                    }
                    FilesEvent::AddToChat {
                        path,
                        is_directory,
                        origin,
                    } => {
                        let payload = WorkspacePathDrag::new(path.clone(), *is_directory)
                            .with_origin(
                                Some(origin.clone()),
                                crate::files::WorkspacePathSource::Tree,
                                None,
                            );
                        let composer = this.composer.clone();
                        this.attach_workspace_drag(&payload, &composer, window, cx);
                    }
                    FilesEvent::Mutate(intent)
                        if this.accepts_file_navigation(&owner, &source, cx) =>
                    {
                        this.start_file_mutation(source.clone(), intent.clone(), cx);
                    }
                    FilesEvent::MutateMany(intents)
                        if this.accepts_file_navigation(&owner, &source, cx) =>
                    {
                        this.start_file_mutations(source.clone(), intents.clone(), cx);
                    }
                    FilesEvent::OpenFile(path)
                        if this.accepts_file_navigation(&owner, &source, cx) =>
                    {
                        this.add_file_surface(path.clone(), window, cx);
                    }
                    FilesEvent::OpenWebLink(activation) => {
                        if let crate::markdown::render::LinkOutcome::External(url) =
                            this.activate_session_link(activation, window, cx)
                        {
                            cx.open_url(&url);
                        }
                    }
                    FilesEvent::ShowAllFilesChanged(show_all) => {
                        this.set_files_show_all(*show_all, cx)
                    }
                    _ => cx.notify(),
                },
            );
            self.files.insert(key.clone(), files);
            self.files_subs.insert(key.clone(), sub);
        }
        push_unique_right_surface(
            self.right_tabs.entry(key.clone()).or_default(),
            RightSurface::Files,
        );
        self.set_surfaces_open(true, cx);
        self.set_right_active(RightSurface::Files, cx);
        if let Some(files) = self.files.get(&key).cloned() {
            files.update(cx, |files, cx| {
                files.ensure_loaded(cx);
                files.focus_explorer(window, cx);
            });
        }
        self.composer
            .update(cx, |composer, _| composer.focus_pending = false);
        cx.notify();
    }

    /// Close the Files tab (the tree keeps its state for the next open).
    #[cfg(feature = "browser-fixture")]
    pub(super) fn close_files_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let has_tab = self
            .right_tabs
            .get(&self.panel_key(cx))
            .is_some_and(|tabs| tabs.contains(&RightSurface::Files));
        if has_tab {
            self.close_right_surface(RightSurface::Files, window, cx);
        }
    }

    /// The Files shortcut: show the tree, or hide the pane while the tree is
    /// what it shows.
    pub(super) fn toggle_files_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.files_tab_shown(cx) && self.horizontal_fit().right {
            self.set_surfaces_open(false, cx);
            window.focus(&self.composer.focus_handle(cx), cx);
            cx.notify();
            return;
        }
        self.add_files_surface(window, cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{AppContext, TestAppContext};

    #[test]
    fn files_layout_shrinks_the_tree_before_the_chat_or_editor() {
        let docked = files_panel_layout(1400.0, 256.0, 286.0, 286.0, 1.0, false);
        assert_eq!(
            docked,
            FilesPanelLayout {
                width: 286.0,
                surface_max: 558.0,
            }
        );
        let narrow = files_panel_layout(1200.0, 256.0, 440.0, 440.0, 1.0, false);
        assert_eq!(
            narrow,
            FilesPanelLayout {
                width: 284.0,
                surface_max: 360.0,
            }
        );
        assert_eq!(
            1200.0 - 256.0 - narrow.width - narrow.surface_max,
            CHAT_PANEL_MIN
        );
        // Closing the surface releases space for the tree. Expanding the
        // surface keeps the tree's existing allocation stable.
        assert_eq!(
            files_panel_layout(1200.0, 256.0, 440.0, 440.0, 0.0, false).width,
            440.0
        );
        let expanded = files_panel_layout(1100.0, 256.0, 286.0, 286.0, 1.0, true);
        assert_eq!(expanded.width, 184.0);
        assert_eq!(expanded.surface_max, 660.0);
        // Growing the viewport restores the preferred width.
        assert_eq!(
            files_panel_layout(1600.0, 256.0, 440.0, 440.0, 1.0, false).width,
            440.0
        );
    }

    #[test]
    fn files_layout_returns_space_smoothly_during_close() {
        let mut previous_chat = 0.0;
        for visible in [186.0, 140.0, 84.0, 40.0, 0.0] {
            let layout = files_panel_layout(1200.0, 256.0, 286.0, visible, 1.0, false);
            let chat = 944.0 - layout.width - layout.surface_max;
            assert!(chat >= previous_chat && chat >= CHAT_PANEL_MIN);
            previous_chat = chat;
        }
        assert_eq!(
            files_panel_layout(1200.0, 256.0, 286.0, 0.0, 1.0, false),
            files_panel_layout(1200.0, 256.0, 0.0, 0.0, 1.0, false),
        );
    }

    #[test]
    fn files_layout_never_takes_space_reserved_for_the_chat_or_surface() {
        for viewport in [0.0, 120.0, 280.0, 600.0, 1000.0, 1200.0, 1600.0] {
            for sidebar in [0.0, 256.0, 400.0] {
                for surfaces in [false, true] {
                    for expanded in [false, true] {
                        for visible in [0.0, 1.0, 140.0, 440.0] {
                            let layout = files_panel_layout(
                                viewport,
                                sidebar,
                                440.0,
                                visible,
                                if surfaces { 1.0 } else { 0.0 },
                                expanded,
                            );
                            let available = (viewport - sidebar).max(0.0);
                            assert!(layout.width >= 0.0 && layout.width <= visible);
                            assert!(layout.surface_max >= 0.0);
                            assert!(layout.width + layout.surface_max <= available + 0.001);
                            if available
                                >= CHAT_PANEL_MIN + if surfaces { RIGHT_PANE_MIN } else { 0.0 }
                            {
                                assert!(
                                    available - layout.width - layout.surface_max >= CHAT_PANEL_MIN
                                        || surfaces && expanded
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    #[gpui::test]
    fn explorer_and_editor_panels_have_independent_session_lifetimes(cx: &mut TestAppContext) {
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
                shell.add_files_surface(window, cx);
                assert!(
                    shell.files.is_empty(),
                    "the new-session canvas has no explorer"
                );
                shell.active_chat = "first".into();
                shell.add_files_surface(window, cx);
                let explorer = shell.files["first"].entity_id();
                // The tree is the right pane's Files tab, never a column.
                assert!(shell.files_tab_shown(cx));
                assert!(!shell.files_column_open(cx));
                assert_eq!(shell.files_reserved_width(cx), 0.0);
                let rows = shell.right_surface_rows(cx);
                assert_eq!(rows.len(), 1);
                assert_eq!(rows[0].0, RightSurface::Files);
                assert_eq!(rows[0].1.as_ref(), "Files");
                // A second open focuses the same singleton tab.
                shell.add_files_surface(window, cx);
                assert_eq!(shell.files["first"].entity_id(), explorer);
                assert_eq!(shell.right_surface_rows(cx).len(), 1);
                // Files open in the main area, beside the conversation.
                shell.add_file_surface("src/main.rs".into(), window, cx);
                shell.add_file_surface("src/main.rs".into(), window, cx);
                assert_eq!(shell.file_surfaces.len(), 1);
                assert_eq!(shell.right_surface_rows(cx).len(), 1);
                assert!(shell.active_main_file(cx).is_some());
                assert_eq!(shell.main_tab_rows(cx).len(), 1);
                assert!(shell.files_tab_shown(cx));
                // The shortcut hides the pane while it shows the tree.
                shell.toggle_files_panel(window, cx);
                assert!(!shell.right_pane_open(cx));
                assert_eq!(shell.file_surfaces.len(), 1);
                assert!(shell.pending_file_closes.is_empty());
                shell.active_chat = "second".into();
                assert!(!shell.files_tab_shown(cx));
                assert!(shell.active_main_file(cx).is_none());
                shell.add_files_surface(window, cx);
                assert_ne!(shell.files["second"].entity_id(), explorer);
                shell.active_chat = "first".into();
                assert!(!shell.files_tab_shown(cx));
                shell.toggle_files_panel(window, cx);
                assert!(shell.files_tab_shown(cx));
                assert_eq!(shell.files["first"].entity_id(), explorer);
                shell.route = Route::Settings(SettingsSection::Files);
                assert!(shell.active_main_file(cx).is_none());
                shell.route = Route::Chat;
                assert!(shell.active_main_file(cx).is_some());
            })
            .unwrap();
    }

    #[gpui::test]
    fn files_open_as_a_pane_tab_and_files_as_main_tabs(cx: &mut TestAppContext) {
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
                shell.active_chat = "chat".into();
                // Widths settle immediately so the assertions see end states.
                shell.reduced_motion = true;
                // A fresh pane toggle lands on the launcher.
                shell.toggle_right_pane(cx);
                assert!(shell.right_pane_open(cx));
                assert_eq!(shell.resolved_right_active(cx), RightSurface::Picker);
                shell.toggle_right_pane(cx);
                assert!(!shell.right_pane_open(cx));

                // Files opens the pane on its tab.
                shell.toggle_files_panel(window, cx);
                assert!(shell.right_pane_open(cx) && shell.files_tab_shown(cx));
                assert_eq!(shell.files_visible_width(cx), 0.0);

                // Opening a file leaves the pane as it was; programmatic opens
                // never close an open pane.
                shell.add_file_surface("src/main.rs".into(), window, cx);
                assert!(shell.files_tab_shown(cx));
                shell.set_surfaces_open(true, cx);
                assert!(shell.files_tab_shown(cx));
                assert_eq!(shell.file_surfaces.len(), 1);

                // Closing the file tab returns the main area to the chat and
                // leaves the pane alone.
                let file = shell.active_main_tab(cx).expect("file tab");
                shell.close_main_tab(file, cx);
                assert!(shell.file_surfaces.is_empty());
                assert!(shell.active_main_tab(cx).is_none());
                assert!(shell.files_tab_shown(cx));

                // Closing the last surface tab (Files) collapses the pane.
                shell.close_right_surface(RightSurface::Files, window, cx);
                assert!(!shell.right_pane_open(cx));
                assert!(shell.right_surface_rows(cx).is_empty());
                // The tree itself is kept for the next open.
                assert!(shell.files.contains_key("chat"));
            })
            .unwrap();
    }
}

#[cfg(all(test, target_os = "linux"))]
#[path = "files_panel_workspace_tests.rs"]
mod workspace_tests;
