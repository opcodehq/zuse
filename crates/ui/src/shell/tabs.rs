//! Conversation tabs share a thread checkout and retain independent agent sessions.
use super::*;
use crate::localization::message as tr;
use zuse_i18n::messages as msg;

const RIGHT_PANE_HEADER_CONTROLS_MIN_WIDTH: f32 = 8.0 + 4.0 + 4.0 + 28.0;

/// The chat one step from `selected` in the sidebar `order`, wrapping at both
/// ends. Pure.
///
/// With nothing selected — the new-session canvas — cycling enters the list at
/// the end it would have wrapped to: the first row going forward, the last
/// going back. A selection that has since left the list (archived from another
/// device mid-cycle) is treated the same way rather than dead-ending.
pub(super) fn cycle_target(
    order: &[String],
    selected: Option<&str>,
    forward: bool,
) -> Option<String> {
    if order.is_empty() {
        return None;
    }
    let at = selected.and_then(|id| order.iter().position(|c| c == id));
    let next = match (at, forward) {
        (Some(at), true) => (at + 1) % order.len(),
        (Some(at), false) => (at + order.len() - 1) % order.len(),
        (None, true) => 0,
        (None, false) => order.len() - 1,
    };
    Some(order[next].clone())
}

/// A thread's top-level conversations split into the strip's open tabs (in
/// strip order: the thread's first conversation, then by creation) and the
/// closed ones its history menu lists (most recently active first). Pure.
///
/// The selected conversation always shows as a tab, even when it was closed
/// earlier and reopened from elsewhere (the sidebar, a deep link), and the
/// strip never empties: with everything closed the first conversation stays.
pub(super) fn split_thread_conversations(
    mut rows: Vec<zuse_protocol::Chat>,
    root: &str,
    closed: &[String],
    selected: &str,
) -> (Vec<zuse_protocol::Chat>, Vec<zuse_protocol::Chat>) {
    rows.sort_by(|a, b| (a.id != root, a.created_at).cmp(&(b.id != root, b.created_at)));
    let (mut open, mut hidden): (Vec<_>, Vec<_>) = rows
        .into_iter()
        .partition(|chat| chat.id == selected || !closed.contains(&chat.id));
    if open.is_empty() && !hidden.is_empty() {
        open.push(hidden.remove(0));
    }
    hidden.sort_by_key(|chat| std::cmp::Reverse(chat.last_message_at.unwrap_or(chat.created_at)));
    (open, hidden)
}

/// The tab to show after closing `closing`: the one to its right, else the
/// one to its left. Pure.
pub(super) fn close_neighbour(open: &[String], closing: &str) -> Option<String> {
    let at = open.iter().position(|id| id == closing)?;
    open.get(at + 1)
        .or_else(|| at.checked_sub(1).and_then(|left| open.get(left)))
        .cloned()
}

/// "1d ago" for the history menu; a just-active conversation reads "now".
fn history_time(at: DateTime<Utc>, now: DateTime<Utc>) -> String {
    match crate::state::format_time_ago(at, now) {
        recent if recent == "now" => recent,
        ago => format!("{ago} ago"),
    }
}

pub(super) fn right_pane_expand_icon(expanded: bool) -> &'static str {
    if expanded {
        icons::COLLAPSE_ARROWS
    } else {
        icons::EXPAND_ARROWS
    }
}

struct PanelTitlebarWidths {
    surface_reveal: f32,
    files_controls: f32,
}

pub(super) struct TitlebarTrailing {
    fit: HorizontalPanelFit,
    right_pane_visible: bool,
    takeover: bool,
    row_left: f32,
    row_gap: f32,
    right_pad: f32,
    widths: PanelTitlebarWidths,
    content_width: f32,
}

/// The fixed right-edge anchor: the 28px pane toggle. Files opens from the
/// pane's launcher list (and its shortcut), not from a titlebar button.
const PANEL_TOGGLE_SLOTS: f32 = 28.0;

fn panel_titlebar_widths(
    surfaces_visible: f32,
    files_visible: f32,
    available: f32,
    right_pad: f32,
) -> PanelTitlebarWidths {
    // Caption controls occupy the far-right panel first. Subtract their
    // clearance once across the combined header, then split it at Files.
    // The explorer and pane toggles keep their slots even when closed.
    let files_controls = (files_visible - right_pad).max(PANEL_TOGGLE_SLOTS);
    let surfaces = surfaces_visible + files_visible - right_pad - files_controls;
    PanelTitlebarWidths {
        surface_reveal: surfaces.min(available - files_controls).max(0.0),
        files_controls,
    }
}

impl Shell {
    /// The same binding navigates the focused pane. Keep the persisted action
    /// IDs so existing user keymaps and the native browser bridge still work.
    pub(super) fn cycle_navigation(
        &mut self,
        forward: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.navigation_overlay_open(cx) {
            return;
        }
        if self.pull_request_pane_open() && self.navigation_focus.in_right(window, cx) {
            self.cycle_pull_request_tabs(forward, window, cx);
        } else if matches!(self.route, Route::Chat)
            && self.right_pane_open(cx)
            && self.navigation_focus.in_right(window, cx)
        {
            let rows = self.right_surface_rows(cx);
            if rows.len() <= 1 {
                return;
            }
            let active = self.resolved_right_active(cx);
            let at = rows.iter().position(|(surface, ..)| *surface == active);
            let next = match (at, forward) {
                (Some(at), true) => (at + 1) % rows.len(),
                (Some(at), false) => (at + rows.len() - 1) % rows.len(),
                (None, true) => 0,
                (None, false) => rows.len() - 1,
            };
            self.activate_right_surface(rows[next].0, window, cx);
        } else {
            self.cycle_session(forward, cx);
        }
    }

    pub(super) fn navigation_overlay_open(&self, cx: &App) -> bool {
        self.overlay_owns_keyboard(cx)
            || self.sync_flow.has_visible_overlay()
            || self.delete_confirm.is_some()
            || self.delete_space_confirm.is_some()
            || self.chat_rename.is_some()
            || self.rename_space_dialog.is_some()
            || self.discard_working_tree.is_some()
            || self.chat_menu.get().is_some()
            || self.pull_request_pane.menu.get().is_some()
            || self.space_menu.get().is_some()
            || self.user_menu.get().is_some()
            || self.spaces_menu.get().is_some()
            || self.right_plus.get().is_some()
            || self.open_in.menu.get().is_some()
            || self.conversation_history.get().is_some()
            || !self.pending_file_closes.is_empty()
            || self.pending_exit.is_some()
    }

    pub(super) fn activate_right_surface(
        &mut self,
        surface: RightSurface,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.navigation_focus.right_was_focused = true;
        self.conversation_composer(cx)
            .update(cx, |composer, _| composer.focus_pending = false);
        // Establish a stable target before detaching the old editor. Read-only
        // surfaces keep it; editable surfaces claim their own focus below/on mount.
        window.focus(&self.navigation_focus.right, cx);
        self.set_right_active(surface, cx);
        self.focus_right_file_editor(surface, window, cx);
    }

    pub(super) fn restore_right_focus_after_close(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.right_pane_open(cx) {
            self.activate_right_surface(self.resolved_right_active(cx), window, cx);
        } else {
            self.navigation_focus.right_was_focused = false;
            window.focus(&self.conversation_composer(cx).focus_handle(cx), cx);
        }
    }

    /// Navigation requests focus once the destination composer renders.
    pub(super) fn focus_composer(&mut self, cx: &mut Context<Self>) {
        self.conversation_composer(cx).update(cx, |composer, cx| {
            composer.focus_pending = true;
            cx.notify();
        });
    }

    /// Ctrl+Tab / Ctrl+Shift+Tab: step through the sidebar's Sessions list in
    /// the order it is drawn. Selection is immediate (no MRU overlay held open
    /// on the modifier) — one press, one session.
    ///
    /// Works from Settings too, landing back in chat like a jump; the
    /// shortcut recorder intercepts these keys while it records. Quiet under
    /// a keyboard-owning overlay: gpui dispatches a matched binding before any
    /// `on_key_down`, so a cycle under the add-space palette would strand the
    /// overlay over a session the user never picked.
    pub(super) fn cycle_session(&mut self, forward: bool, cx: &mut Context<Self>) {
        if self.overlay_owns_keyboard(cx) {
            return;
        }
        // The same list `render_active_rows` draws and the jump shortcuts
        // count — one function, so neither can drift from the screen.
        let order = self.sidebar_visible_order(cx);
        let selected = self.state.read(cx).selected_chat.clone();
        if let Some(target) = cycle_target(&order, selected.as_deref(), forward) {
            self.open_chat(target, cx);
        }
    }

    /// Boot landing: the most recently active visible chat once the first
    /// chats frame has synced (manual selection wins; no chats → the
    /// new-session canvas shows).
    pub(super) fn boot_select_chat(&mut self, cx: &mut Context<Self>) {
        if self.cloud_conversation.is_some() {
            return;
        }
        let first = {
            let state = self.state.read(cx);
            if !state.chats_synced || state.selected_chat.is_some() || state.auto_selected {
                return;
            }
            state
                .overview_chats(Utc::now())
                .first()
                .map(|(_, c)| c.id.clone())
        };
        if let Some(first) = first {
            self.focus_composer(cx);
            self.state
                .update(cx, |s, cx| s.select_chat(Some(first), cx));
        }
    }

    /// Open a session from the sidebar: select it, the main area follows.
    pub(crate) fn open_chat(&mut self, chat_id: String, cx: &mut Context<Self>) {
        self.cloud_conversation = None;
        self.cloud_conversation_events = None;
        self.command_palette = None;
        // Opening any session steps the voice stage aside — including the one
        // already selected under it. The call keeps running in the background.
        self.set_voice_stage_open(false, cx);
        self.set_route(Route::Chat, cx);
        self.focus_composer(cx);
        self.nav.push(NavEntry::Chat(chat_id.clone()));
        self.state
            .update(cx, |s, cx| s.select_chat(Some(chat_id), cx));
        cx.notify();
    }

    /// `+` in the titlebar: open the new-session canvas. A set sidebar filter
    /// re-homes the canvas onto that project; under "All" the canvas reopens
    /// on the project/device it was left at (see `AppState::canvas_target`).
    ///
    /// A new chat always starts with the terminal hidden: when the drawer is
    /// open it just hides (detach, not close — the source chat's tabs and
    /// PTYs survive for the return trip).
    ///
    /// `project` (the per-project `+` on a sidebar group header) homes the
    /// canvas on that project and wins over the sidebar filter.
    pub(super) fn open_new_session(&mut self, project: Option<String>, cx: &mut Context<Self>) {
        self.sync_cloud_tab_scope(cx);
        if !self.cloud_tab_navigation_ready(cx) {
            return;
        }
        if project.is_none() {
            let cloud_only = {
                let state = self.state.read(cx);
                // Only launch choices filter unavailable repositories. The
                // sidebar retains all projects and their existing history.
                let projects: Vec<_> = state
                    .cloud_sidebar_projects()
                    .into_iter()
                    .filter(|project| project["canCreate"] != false)
                    .collect();
                let remembered = state.cloud_project.as_ref().and_then(|selected| {
                    projects
                        .iter()
                        .find(|row| {
                            row["projectId"] == selected["projectId"]
                                && row["accountId"] == selected["accountId"]
                        })
                        .cloned()
                });
                let explicit_local = self
                    .settings
                    .space_filter
                    .as_deref()
                    .and_then(|id| state.space_row(id))
                    .or_else(|| state.selected_space_row())
                    .is_some_and(|space| {
                        !zuse_protocol::projectless::is_projectless_cwd(Some(&space.path))
                    });
                if explicit_local {
                    None
                } else {
                    remembered.or_else(|| {
                        (!state.spaces.iter().any(|space| {
                            !zuse_protocol::projectless::is_projectless_cwd(Some(&space.path))
                        }))
                        .then(|| projects.into_iter().next())
                        .flatten()
                    })
                }
            };
            if let Some(project) = cloud_only {
                self.open_new_cloud_session(project, cx);
                return;
            }
        }
        self.reset_new_session_canvas(project, cx);
    }

    /// Reuse the current native conversation surface; callers choose its destination.
    pub(super) fn reset_new_session_canvas(
        &mut self,
        project: Option<String>,
        cx: &mut Context<Self>,
    ) {
        if !self.cloud_tab_navigation_ready(cx) {
            return;
        }
        self.cloud_tabs.clear_current();
        self.cloud_conversation = None;
        self.cloud_conversation_events = None;
        self.command_palette = None;
        self.set_voice_stage_open(false, cx);
        self.set_route(Route::Chat, cx);
        self.focus_composer(cx);
        // Pre-hide before the selection flips so the state change can't
        // auto-create a canvas tab (the panel's observer runs on the same
        // update). The source chat's flag stays set — returning restores it.
        let was_open = self.terminal_open(cx)
            || self
                .terminal
                .as_ref()
                .is_some_and(|panel| panel.read(cx).is_open());
        if was_open {
            self.terminal_tween = None;
            if let Some(panel) = self.terminal.clone() {
                panel.update(cx, |panel, cx| panel.set_open(false, cx));
            }
        }
        self.nav.push(NavEntry::Chat(String::new()));
        let target = {
            let state = self.state.read(cx);
            project
                .or_else(|| self.settings.space_filter.clone())
                .filter(|id| state.space_row(id).is_some())
        };
        let defaults = crate::settings::composer::ComposerDefaults::load(&self.data_dir);
        self.state.update(cx, |s, cx| {
            // Leaving a chat puts the canvas's own target back; the filter
            // (an explicit standing choice) then applies on top of it.
            let restores = s.canvas_target.is_some();
            s.select_chat(None, cx);
            if target.is_some() {
                s.select_space(target, cx);
            } else if !restores && defaults.no_project {
                // No canvas target was set aside (e.g. after a runtime swap):
                // fall back to the saved new-session opt-out.
                s.select_space(None, cx);
                if let Some(device) = defaults.device {
                    s.select_device(device, cx);
                }
            }
        });
        // The canvas never restores a drawer: a previously opened canvas
        // terminal must not pop open on a fresh new chat.
        let key = self.panel_key(cx);
        if self.panels.get(&key).terminal_open {
            self.panels.update(&key, |panels| {
                panels.terminal_open = false;
            });
        }
        self.terminal_tween = None;
        if let Some(panel) = self.terminal.clone()
            && panel.read(cx).is_open()
        {
            panel.update(cx, |panel, cx| panel.set_open(false, cx));
        }
        cx.notify();
        self.initialize_new_cloud_canvas(cx);
    }

    /// The trailing titlebar strip follows the columns beneath it.
    pub(super) fn titlebar_trailing_layout(&self, on_canvas: bool, cx: &App) -> TitlebarTrailing {
        let sidebar_now = self.sidebar_now();
        let plus_inset = TITLEBAR_ACTION_SLOT_WIDTH * self.titlebar_plus_alpha(cx);

        // Same glide as the old strip: content starts at the inset card's
        // left edge while the sidebar is open, and slides toward the control
        // cluster as it collapses.
        let content_left =
            (sidebar_now + Theme::SPACE_LG).max(self.title_bar_content_start() + plus_inset);

        // Trailing titlebar section. With the changes pane open this is the
        // PANE'S HEADER — a strip exactly as wide as the pane carrying its
        // controls (scope dropdown, ref selector, fold-all from the Changes
        // entity; expand + close shell-side). It lives up here because the
        // titlebar overlay owns this band's hit-testing: controls mounted in
        // the pane itself would sit under the drag region and never see a
        // click. Closed, it is just the stable open/close toggle. Hidden on
        // the new-session canvas (user request) — nothing to diff yet.
        let fit = self.horizontal_fit();
        let right_pane_visible = !on_canvas
            && ((fit.right && (self.right_pane_open(cx) || self.tween_active(self.right_tween)))
                || self.fit_exit_width(1).is_some());
        let takeover = right_pane_visible
            && (self.right_pane_expanded || self.tween_active(self.main_takeover_tween));
        // In takeover the title hides and the strip owns the whole band, so
        // the row's left inset pulls back to the sidebar seam — the title
        // inset would push the scope dropdown off the pane's own left gutter
        // (user report: misaligned dead space). With the sidebar COLLAPSED
        // the seam is the window edge, where the traffic lights + nav
        // cluster overlay lives — the strip must still clear it, but only
        // just: `title_bar_content_start` carries the identity-group margin the
        // strip doesn't want (it brings its own 8px pad), and doubling up
        // read as a hole after the `+` (user report).
        let row_left = if takeover {
            // The surface tabs must LEFT-ALIGN with the pane's own rows (the
            // diff options and stats strip carry an 8px box gutter off the
            // seam — user report: rows started at different insets). The
            // strip's width is capped to `avail`, which subtracts the row's
            // 8px child gap — pulling row_left 8 LEFT of the seam cancels
            // that, so the uncapped strip starts exactly at the seam and its
            // own 8px pad lands the first chip on the pane gutter. The
            // window-control cluster still wins while the sidebar is
            // collapsed (the chips clear it instead of underlapping).
            let cluster_end =
                self.title_bar_content_start() - TITLEBAR_IDENTITY_GAP + plus_inset - 14.0;
            (sidebar_now - 8.0).max(cluster_end)
        } else {
            content_left
        };
        let row_gap = 8.0;
        let files_width = self.files_visible_width(cx);
        let right_pad = self.titlebar_right_pad(TITLEBAR_ACTION_EDGE_INSET);
        // The title row's gaps are outside the fixed-width panel controls.
        let gap_budget = if takeover { row_gap } else { row_gap * 3.0 };
        // Match the surface's visible width, including its resize bounce,
        // so the header follows the pane seam as the sidebar moves.
        let right_visible = self.right_visible_width(cx);
        let widths = panel_titlebar_widths(
            right_visible,
            files_width,
            self.viewport_width - row_left - right_pad - gap_budget,
            right_pad,
        );
        // Match the pane's stable inner width during open/close. Reflowing
        // the scroller on every masked frame clamps its scroll offset and
        // makes the tabs jump when the close is reversed.
        let content_width = panel_titlebar_widths(
            self.right_content_width(self.right_target(cx)),
            files_width,
            self.viewport_width - row_left - right_pad - gap_budget,
            right_pad,
        )
        .surface_reveal
        .max(widths.surface_reveal);
        TitlebarTrailing {
            fit,
            right_pane_visible,
            takeover,
            row_left,
            row_gap,
            right_pad,
            widths,
            content_width,
        }
    }

    /// The strip's `+`: a new conversation in the selected thread, sharing
    /// its checkout, opened as the active tab.
    pub(super) fn add_thread_conversation(&mut self, cx: &mut Context<Self>) {
        if self.thread_conversation_creating {
            return;
        }
        let state = self.state.read(cx);
        let Some(selected) = state.selected_chat_row().filter(|chat| !chat.archived) else {
            return;
        };
        let Some(engine) = state.engine().cloned() else {
            return;
        };
        let root = selected.thread_id().to_owned();
        let device = selected.device_id.clone();
        self.thread_conversation_creating = true;
        cx.spawn(async move |this, cx| {
            let result = engine.client().call_as::<zuse_protocol::Chat>(methods::CREATE_THREAD_SESSION,
                serde_json::json!({"chatId": uuid::Uuid::new_v4().to_string(), "threadId": root, "targetDeviceId": device})).await;
            let _ = this.update(cx, |this, cx| {
                this.thread_conversation_creating = false;
                match result {
                    Ok(chat) => {
                        let id = chat.id.clone();
                        this.state.update(cx, |state, _| {
                            if !state.chats.iter().any(|row| row.id == id) { state.chats.push(chat); }
                        });
                        this.open_chat(id, cx);
                    }
                    Err(error) => this.composer.update(cx, |composer, cx| composer.show_error(error.to_string(), cx)),
                }
                cx.notify();
            });
        }).detach();
        cx.notify();
    }

    /// The selected thread's conversations as `(thread id, open tabs, closed)`.
    pub(super) fn thread_conversation_tabs(
        &self,
        cx: &App,
    ) -> Option<(String, Vec<zuse_protocol::Chat>, Vec<zuse_protocol::Chat>)> {
        let state = self.state.read(cx);
        let selected = state.selected_chat_row()?;
        let root = selected.thread_id().to_owned();
        let rows: Vec<_> = state
            .chats
            .iter()
            .filter(|chat| {
                chat.thread_id() == root && chat.parent_chat_id.is_none() && !chat.archived
            })
            .cloned()
            .collect();
        let closed = self
            .settings
            .closed_thread_conversations
            .get(&root)
            .map(Vec::as_slice)
            .unwrap_or_default();
        let (open, hidden) = split_thread_conversations(rows, &root, closed, &selected.id);
        Some((root, open, hidden))
    }

    /// Close a conversation tab: it leaves the strip but stays in the thread,
    /// listed in the history menu. The last open tab never closes; closing
    /// the one on screen shows its neighbour.
    pub(super) fn close_thread_conversation(&mut self, chat_id: &str, cx: &mut Context<Self>) {
        let Some((root, open, _)) = self.thread_conversation_tabs(cx) else {
            return;
        };
        let open: Vec<String> = open.into_iter().map(|chat| chat.id).collect();
        if open.len() <= 1 {
            return;
        }
        let Some(neighbour) = close_neighbour(&open, chat_id) else {
            return;
        };
        let closed = self
            .settings
            .closed_thread_conversations
            .entry(root)
            .or_default();
        if !closed.iter().any(|id| id == chat_id) {
            closed.push(chat_id.to_owned());
        }
        self.schedule_save(cx);
        if self.state.read(cx).selected_chat.as_deref() == Some(chat_id) {
            self.open_chat(neighbour, cx);
        }
        cx.notify();
    }

    /// Reopen a closed conversation from the history menu as the active tab.
    fn restore_thread_conversation(&mut self, chat_id: String, cx: &mut Context<Self>) {
        if let Some((root, ..)) = self.thread_conversation_tabs(cx)
            && let Some(closed) = self.settings.closed_thread_conversations.get_mut(&root)
        {
            closed.retain(|id| *id != chat_id);
            if closed.is_empty() {
                self.settings.closed_thread_conversations.remove(&root);
            }
            self.schedule_save(cx);
        }
        self.close_conversation_history(cx);
        self.open_chat(chat_id, cx);
        self.activate_main_tab(None, cx);
    }

    pub(super) fn close_conversation_history(&mut self, cx: &mut Context<Self>) {
        if self.conversation_history.begin_close() {
            popover::reap_popup(cx, |shell: &mut Self| &mut shell.conversation_history);
        }
    }

    pub(super) fn render_thread_conversations(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = Theme::of(cx).clone();
        let Some((_, rows, closed_rows)) = self.thread_conversation_tabs(cx) else {
            return Empty.into_any_element();
        };
        let state = self.state.read(cx);
        let Some(selected) = state.selected_chat_row() else {
            return Empty.into_any_element();
        };
        // Side chats of the open conversation list after its siblings; they
        // open in the right pane, beside the conversation they branch from.
        let mut side_chats: Vec<_> = state
            .chats
            .iter()
            .filter(|chat| {
                chat.parent_chat_id.as_deref() == Some(selected.id.as_str()) && !chat.archived
            })
            .cloned()
            .collect();
        side_chats.sort_by_key(|chat| chat.created_at);
        // Tabs whose agent is mid-turn trade their harness icon for the
        // activity glyph, so work in a background tab stays visible.
        let now = Utc::now();
        let working: std::collections::HashSet<String> = rows
            .iter()
            .chain(&side_chats)
            .filter(|chat| {
                state.display_status_for(chat, now) == zuse_protocol::ChatIndicator::Working
            })
            .map(|chat| chat.id.clone())
            .collect();
        let selected_id = selected.id.clone();
        let archived = selected.archived;
        let main_tab = self.active_main_tab(cx);
        let main_tabs = self.main_tab_rows(cx);
        let closable = rows.len() > 1;
        let tab_spinner = |id: &str, cx: &mut Context<Self>| {
            div()
                .size(px(14.))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .child(crate::loaders::mini_glyph_spinner(
                    format!("thread-conversation-working-{id}"),
                    2.0,
                    theme.glyph,
                    cx.entity_id(),
                    cx,
                ))
                .into_any_element()
        };
        let tab_chip = |id: SharedString, title: SharedString, active: bool| {
            div()
                .id(id)
                .role(gpui::Role::Tab)
                .aria_label(title)
                .aria_selected(active)
                .cursor_pointer()
                .flex_none()
                .w(px(160.))
                .h(px(24.))
                .px(px(8.))
                .flex()
                .items_center()
                .gap(px(6.))
                .rounded(px(7.))
                .text_size(crate::typography::text_xs())
                .when(active, |d| {
                    d.bg(theme.accent_fill())
                        .text_color(theme.text_strong)
                        .font_weight(gpui::FontWeight::MEDIUM)
                })
                .when(!active, |d| {
                    d.text_color(theme.text_muted)
                        .hover(|d| d.bg(theme.muted_fill().opacity(0.6)).text_color(theme.text))
                })
        };
        // The × every closable tab shares: shown on the active tab, revealed
        // on hover for the rest.
        let close_button =
            |id: SharedString, label: &'static str, active: bool, group: SharedString| {
                div()
                    .id(id)
                    .role(gpui::Role::Button)
                    .aria_label(label)
                    .size(px(16.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(4.))
                    .when(!active, |d| d.invisible())
                    .group_hover(group, |d| d.visible())
                    .hover(|d| d.bg(theme.muted_fill()))
                    .child(
                        icon(icons::CLOSE)
                            .size(px(10.))
                            .text_color(theme.text_muted),
                    )
            };
        let main_chips = main_tabs
            .into_iter()
            .map(|row| {
                let main_tabs::MainTabRow {
                    tab,
                    title,
                    review,
                    dirty,
                    path,
                } = row;
                let active = main_tab == Some(tab);
                // A file tab drags into a composer as a file mention.
                let drag = match tab {
                    main_tabs::MainTab::File(id) => {
                        self.workspace_path_for_surface(RightSurface::File(id), cx)
                    }
                    main_tabs::MainTab::Diff(_) => None,
                };
                let key = match tab {
                    main_tabs::MainTab::Diff(id) => format!("diff-{id}"),
                    main_tabs::MainTab::File(id) => format!("file-{id}"),
                };
                let group = SharedString::from(format!("main-tab-{key}"));
                let selector = group.to_string();
                let leading = match &path {
                    Some(path) => crate::file_icons::icon(
                        crate::file_icons::FileIconIdentity::file(path.as_ref()),
                        theme.appearance,
                    )
                    .size(px(14.))
                    .flex_none()
                    .into_any_element(),
                    None => icon(if review {
                        icons::GIT_COMPARE
                    } else {
                        icons::GIT_BRANCH
                    })
                    .size(px(13.))
                    .flex_none()
                    .text_color(theme.text_muted)
                    .into_any_element(),
                };
                let close_label = match tab {
                    main_tabs::MainTab::File(_) => "Close file",
                    main_tabs::MainTab::Diff(_) if review => "Close review",
                    main_tabs::MainTab::Diff(_) => "Close diff",
                };
                // An unsaved file keeps a dot in the close slot until hovered.
                let close = close_button(
                    SharedString::from(format!("main-tab-close-{key}")),
                    close_label,
                    active || dirty,
                    group.clone(),
                )
                .relative()
                .when(dirty, |slot| {
                    slot.child(
                        div()
                            .absolute()
                            .inset_0()
                            .flex()
                            .items_center()
                            .justify_center()
                            .bg(if active {
                                theme.accent_fill()
                            } else {
                                theme.surface
                            })
                            .rounded(px(4.))
                            .group_hover(group.clone(), |d| d.opacity(0.))
                            .child(div().size(px(6.)).rounded_full().bg(theme.text_muted)),
                    )
                })
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.close_main_tab(tab, cx);
                }));
                tab_chip(group.clone(), title.clone(), active)
                    .group(group.clone())
                    .pr(px(4.))
                    .debug_selector(move || selector.clone())
                    .when_some(path, |chip, path| {
                        chip.tooltip(crate::settings::widgets::text_tooltip(path))
                            .tooltip_show_delay(std::time::Duration::from_millis(350))
                    })
                    .when_some(drag, |chip, drag| {
                        chip.on_drag(drag, |payload, _, _, cx| {
                            cx.stop_propagation();
                            crate::files::workspace_path_drag_ghost(payload, cx)
                        })
                    })
                    .child(leading)
                    .child(div().flex_1().min_w_0().truncate().child(title))
                    .child(close)
                    .on_mouse_down(
                        MouseButton::Middle,
                        cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.close_main_tab(tab, cx);
                        }),
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.focus_main_tab(Some(tab), window, cx)
                    }))
                    .into_any_element()
            })
            .collect::<Vec<_>>();
        let conversation_chips = rows
            .into_iter()
            .enumerate()
            .map(|(index, chat)| {
                let active = chat.id == selected_id && main_tab.is_none();
                let title = chat
                    .title
                    .clone()
                    .unwrap_or_else(|| format!("Chat {}", index + 1));
                let brand = chat
                    .config
                    .as_ref()
                    .map(|c| crate::pickers::harness_brand_icon(c.harness));
                let id = chat.id;
                let group = SharedString::from(format!("thread-conversation-{id}"));
                let (close_id, middle_id) = (id.clone(), id.clone());
                tab_chip(group.clone(), title.clone().into(), active)
                    .group(group.clone())
                    .when(closable, |d| d.pr(px(4.)))
                    .debug_selector({
                        let id = id.clone();
                        move || format!("thread-conversation-{id}")
                    })
                    .child(if working.contains(&id) {
                        tab_spinner(&id, cx)
                    } else {
                        match brand {
                            Some((path, tint)) => icon(path)
                                .size(px(14.))
                                .flex_none()
                                .text_color(tint.unwrap_or(theme.text_muted))
                                .into_any_element(),
                            None => icon(icons::CHAT_ROUND_LINE)
                                .size(px(14.))
                                .flex_none()
                                .text_color(theme.text_muted)
                                .into_any_element(),
                        }
                    })
                    .child(div().flex_1().min_w_0().truncate().child(title))
                    .when(closable, |d| {
                        d.child(
                            close_button(
                                SharedString::from(format!("thread-conversation-close-{id}")),
                                "Close chat",
                                active,
                                group,
                            )
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.close_thread_conversation(&close_id, cx);
                                },
                            )),
                        )
                        .on_mouse_down(
                            MouseButton::Middle,
                            cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.close_thread_conversation(&middle_id, cx);
                            }),
                        )
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if this.active_chat == id {
                            this.activate_main_tab(None, cx);
                        } else {
                            this.open_chat(id.clone(), cx);
                        }
                    }))
                    .into_any_element()
            })
            .collect::<Vec<_>>();
        let side_chips = side_chats
            .into_iter()
            .map(|chat| {
                let title: SharedString = chat
                    .title
                    .clone()
                    .or_else(|| chat.last_message_preview.clone())
                    .unwrap_or_else(|| "Side chat".into())
                    .into();
                let brand = chat
                    .config
                    .as_ref()
                    .map(|c| crate::pickers::harness_brand_icon(c.harness));
                let id = chat.id;
                let rename = self.rename_input_for(&id, ChatRenameSurface::Tab);
                let (open_id, menu_id, rename_id) = (id.clone(), id.clone(), id.clone());
                tab_chip(
                    SharedString::from(format!("side-chat-tab-{id}")),
                    title.clone(),
                    false,
                )
                .child(if working.contains(&id) {
                    tab_spinner(&id, cx)
                } else {
                    icon(brand.map_or(icons::CHAT_ROUND_LINE, |(path, _)| path))
                        .size(px(14.))
                        .text_color(brand.and_then(|(_, tint)| tint).unwrap_or(theme.text_muted))
                        .into_any_element()
                })
                .child(match rename {
                    Some(input) => div().w(px(120.)).child(input).into_any_element(),
                    None => div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .child(title)
                        .into_any_element(),
                })
                .on_click(cx.listener(move |this, event: &gpui::ClickEvent, _, cx| {
                    if event.click_count() >= 2 {
                        this.open_rename_chat(rename_id.clone(), cx);
                    } else {
                        this.open_child_chat_tab(&open_id, cx);
                    }
                }))
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(move |this, event: &gpui::MouseDownEvent, _, cx| {
                        cx.stop_propagation();
                        this.chat_menu.open(ChatMenuState {
                            chat_id: menu_id.clone(),
                            tab: None,
                            position: event.position,
                            page: ChatMenuPage::Root,
                        });
                        cx.notify();
                    }),
                )
                .into_any_element()
            })
            .collect::<Vec<_>>();
        // Quiet 24px strip buttons: the `+` after the last tab and the
        // history clock at the strip's right end.
        let strip_button = |id: &'static str, glyph: &'static str, label: &'static str| {
            div()
                .id(id)
                .role(gpui::Role::Button)
                .aria_label(label)
                .size(px(24.))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(7.))
                .cursor_pointer()
                .hover(|d| d.bg(theme.muted_fill().opacity(0.6)))
                .tooltip(crate::settings::widgets::text_tooltip(label))
                .child(icon(glyph).size(px(14.)).text_color(theme.text_muted))
        };
        let history_open = self.conversation_history.get().is_some();
        let history = strip_button(
            "thread-conversation-history",
            icons::HISTORY,
            "Earlier chats",
        )
        .aria_expanded(history_open)
        .when(history_open, |d| d.bg(theme.muted_fill().opacity(0.6)))
        .block_mouse_except_scroll()
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _, window, _| {
                window.prevent_default();
                this.conversation_history.note_trigger_press();
            }),
        )
        .on_click(cx.listener(|this, _, _, cx| {
            cx.stop_propagation();
            if this.conversation_history.take_press_was_open() {
                this.close_conversation_history(cx);
            } else {
                this.conversation_history.open(());
                cx.notify();
            }
        }));
        let history = if history_open {
            let closing = self.conversation_history.closing_since();
            let menu = self.render_conversation_history(&closed_rows, &theme, cx);
            history
                .relative()
                .child(popover::anchored_menu_below_end(
                    "thread-conversation-history-menu",
                    menu,
                    closing,
                ))
                .into_any_element()
        } else {
            history.into_any_element()
        };
        div()
            .w_full()
            .flex()
            .items_center()
            .min_w_0()
            .gap(px(4.))
            .child(
                div()
                    .id("thread-conversations")
                    .flex()
                    .items_center()
                    .min_w_0()
                    .overflow_x_scroll()
                    .gap(px(4.))
                    .children(main_chips)
                    .children(conversation_chips)
                    .children(side_chips),
            )
            .when(!archived, |d| {
                d.child(
                    strip_button(
                        "add-thread-conversation",
                        icons::PLUS,
                        "New chat in this thread",
                    )
                    .on_click(cx.listener(|this, _, _, cx| this.add_thread_conversation(cx))),
                )
            })
            .child(div().flex_1())
            .child(history)
            .into_any_element()
    }

    /// The history menu: this thread's closed conversations, most recent
    /// first; a row reopens its conversation as the active tab.
    fn render_conversation_history(
        &self,
        closed: &[zuse_protocol::Chat],
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = &theme.for_popup();
        let now = Utc::now();
        let rows = closed.iter().map(|chat| {
            let brand = chat
                .config
                .as_ref()
                .map(|c| crate::pickers::harness_brand_icon(c.harness));
            let title: SharedString = chat
                .title
                .clone()
                .or_else(|| chat.last_message_preview.clone())
                .unwrap_or_else(|| "Untitled chat".into())
                .into();
            let id = chat.id.clone();
            let fade = format!("thread-history-row-{id}");
            popover::menu_row(theme, false, fade.clone())
                .id(SharedString::from(fade))
                .role(gpui::Role::MenuItem)
                .aria_label(title.clone())
                .child(
                    icon(brand.map_or(icons::CHAT_ROUND_LINE, |(path, _)| path))
                        .size(px(14.))
                        .flex_none()
                        .text_color(brand.and_then(|(_, tint)| tint).unwrap_or(theme.text_muted)),
                )
                .child(div().flex_1().min_w_0().truncate().child(title))
                .child(
                    div()
                        .flex_none()
                        .text_size(crate::typography::text_xs())
                        .text_color(theme.text_muted)
                        .child(history_time(
                            chat.last_message_at.unwrap_or(chat.created_at),
                            now,
                        )),
                )
                .child(
                    icon(icons::RESTORE)
                        .size(px(13.))
                        .flex_none()
                        .text_color(theme.text_muted),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.restore_thread_conversation(id.clone(), cx)
                }))
                .into_any_element()
        });
        popover::popover_card(theme)
            .w(px(300.))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| this.close_conversation_history(cx)))
            .flex()
            .flex_col()
            .when(closed.is_empty(), |d| {
                d.child(
                    div()
                        .px(px(8.))
                        .py(px(6.))
                        .text_color(theme.text_muted)
                        .child(tr(cx, msg::native::no_earlier_chats)),
                )
            })
            .children(rows)
            .into_any_element()
    }

    pub(super) fn render_session_title_bar(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = Theme::of(cx).clone();
        // The canvas titles as NOTHING (user request — a "New session"
        // header over the empty canvas was noise); the bar keeps its height,
        // drag region, and buttons. Where a session runs, and its side-chat
        // and project actions, live in the session card.
        let on_canvas = !self.has_conversation(cx);

        // The new-session `+` renders in the WINDOW-CONTROL CLUSTER whenever a
        // session is selected (`render_titlebar_cluster`) — this row budgets
        // one button slot so the title never sits under it.
        let TitlebarTrailing {
            fit,
            right_pane_visible,
            takeover,
            row_left,
            row_gap,
            right_pad,
            widths,
            content_width,
        } = self.titlebar_trailing_layout(on_canvas, cx);

        let trailing: Option<gpui::AnyElement> = if on_canvas {
            None
        } else {
            let mut controls = div()
                .id("right-titlebar-controls")
                .debug_selector(|| "right-titlebar-controls".into())
                .flex_none()
                .h_full()
                .flex()
                .flex_row()
                .items_center()
                // The transcript extends under this band (it fades beneath
                // the tab strip); a wheel over the tabs must scroll the
                // strip, never the surface behind it.
                .on_scroll_wheel(|_, _, cx| cx.stop_propagation());
            if right_pane_visible {
                // The right pane's SURFACE TABS — the diff
                // options that used to live here moved into the pane's own
                // second row; expand stays in this band (user request).
                let show_pane_controls =
                    widths.surface_reveal >= RIGHT_PANE_HEADER_CONTROLS_MIN_WIDTH;
                // The toggle is the fixed right-edge anchor, like the left
                // sidebar control. Only the tabs + expand section reveals to
                // its left; including the toggle in this animated width
                // compressed both icons into the same clipped box at open.
                controls = controls.child(
                    div()
                        .w(px(widths.surface_reveal))
                        .h_full()
                        .flex_none()
                        .relative()
                        .overflow_hidden()
                        .child(
                            div()
                                .debug_selector(|| "right-titlebar-content".into())
                                .absolute()
                                .right_0()
                                .top_0()
                                .w(px(content_width))
                                .h_full()
                                .flex()
                                .flex_row()
                                .items_center()
                                .gap(px(4.0))
                                .overflow_hidden()
                                // 8 + the trigger's own 8px pad = the pane's 16px
                                // text gutter. The 4px right padding is the stable
                                // gap before the fixed toggle.
                                .when(show_pane_controls, |el| {
                                    el.pl(px(8.0))
                                        .pr(px(4.0))
                                        .child(
                                            div()
                                                .flex_1()
                                                .min_w_0()
                                                .h_full()
                                                .overflow_hidden()
                                                .child(self.render_right_tab_strip(cx)),
                                        )
                                        .child(header_icon_button(
                                            "expand-changes",
                                            right_pane_expand_icon(self.right_pane_expanded),
                                            if self.right_pane_expanded {
                                                "Collapse panel"
                                            } else {
                                                "Expand panel"
                                            },
                                            &theme,
                                            cx.listener(|this, _, _, cx| {
                                                this.toggle_right_pane_expand(cx)
                                            }),
                                        ))
                                }),
                        ),
                );
            }
            // The explorer slot sits over the explorer column and carries the
            // fixed right-edge anchor — the pane toggle — which stays mounted
            // at one position while the surface tabs reveal to its left. The
            // explorer's own search and visibility controls live in its
            // secondary header.
            Some(
                controls
                    .child(
                        div()
                            .w(px(widths.files_controls))
                            .h_full()
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_end()
                            // No hairline here: the explorer column below is
                            // padded down by the titlebar height and its left
                            // border already runs through this band, so a
                            // second one on the slot stacked on the same
                            // pixels and read lighter than the seam beneath
                            // it (user report).
                            .child(header_icon_button_with(
                                "toggle-changes",
                                icons::sidebar_glyph(
                                    motion::state_t(
                                        "toggle-changes",
                                        self.right_pane_open(cx) && fit.right,
                                        motion::GLYPH_STATE,
                                        self.reduced_motion,
                                    ),
                                    true,
                                    14.0,
                                    theme.text_muted,
                                ),
                                ShortcutId::ToggleChanges.label(),
                                cx.listener(|this, _, _, cx| this.toggle_right_pane(cx)),
                            )),
                    )
                    .into_any_element(),
            )
        };

        // The session card's toggle belongs to the transcript: it ends the
        // transcript's stretch of the titlebar, left of the side pane's tabs
        // when one is open, else just left of the explorer toggle.
        // The conversation's own top-bar actions, in the reference order:
        // Open in, the environment summary, then the bottom terminal; the
        // right pane's toggle anchors the far edge.
        let session_info_toggle = (!takeover && !on_canvas).then(|| {
            let open = self.session_info_visible();
            header_icon_button(
                "toggle-session-info",
                icons::LIST,
                "Toggle environment summary",
                &theme,
                cx.listener(|this, _, _, cx| this.toggle_session_info(cx)),
            )
            .role(gpui::Role::Button)
            .aria_label(tr(cx, msg::chat::top_bar_toggle_environment_summary))
            .aria_selected(open)
            .flex_none()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, _| this.note_session_info_press()),
            )
            .when(open, |button| button.bg(theme.text.opacity(0.10)))
        });
        let open_in = if !takeover && !on_canvas {
            self.render_open_in(&theme, cx)
        } else {
            None
        };
        let terminal_toggle = (!takeover && !on_canvas && self.cloud_conversation.is_none()).then(|| {
            let open = self.terminal_open(cx);
            header_icon_button(
                "toggle-bottom-terminal",
                icons::PANEL_BOTTOM,
                "Toggle bottom terminal",
                &theme,
                cx.listener(|this, _, window, cx| this.toggle_terminal(window, cx)),
            )
            .role(gpui::Role::Button)
            .aria_label(tr(cx, msg::common::toggle_bottom_terminal))
            .aria_selected(open)
            .flex_none()
            .when(open, |button| button.bg(theme.text.opacity(0.10)))
        });
        // The left cluster: owner avatar, branch and copy (reference top bar).
        let identity = if !takeover && !on_canvas {
            self.render_top_bar_branch(&theme, cx)
        } else {
            None
        };
        let cloud_info = if !takeover && !on_canvas {
            self.render_cloud_info(&theme, cx)
        } else {
            None
        };
        let inner = div()
            .size_full()
            .flex()
            .items_center()
            .pt(px(Theme::TITLEBAR_TOP_PAD))
            .gap(px(row_gap))
            .pl(px(row_left))
            .pr(px(right_pad))
            // In panel takeover the header strip spans the whole band — the
            // identity would sit UNDER it (both flex_none, the row overflows
            // and paint order stacks them), so it hides for the duration.
            // The conversation tabs live on their own row below this bar
            // (`render_conversation_tab_row`).
            .when(!takeover, |el| {
                el.child(
                    div()
                        .min_w_0()
                        .flex_1()
                        .overflow_hidden()
                        .flex()
                        .flex_row()
                        .items_center()
                        .children(identity),
                )
            })
            .when(takeover, |d| d.child(div().flex_1()))
            .child(
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .children(cloud_info)
                    .children(open_in)
                    .children(session_info_toggle)
                    .children(terminal_toggle),
            )
            .children(trailing);

        // The unified window titlebar: full-width on the glass shell, ABOVE
        // the inset card. No bottom border — the card's own hairline is the
        // separation; the glass gutter shows between.
        let bar = div().h(px(Theme::TITLEBAR_HEIGHT)).flex_none().child(inner);
        self.titlebar_drag_region("chat-titlebar", bar, cx)
            .into_any_element()
    }
}

#[cfg(test)]
mod panel_titlebar_tests {
    use super::*;

    #[test]
    fn tabs_align_with_the_panel_for_each_caption_layout_and_files_width() {
        let viewport = 1400.0;
        for right_pad in [6.0, 40.0, 92.0, 114.0] {
            for files in [0.0, 10.0, 28.0, 100.0, 220.0, 286.0, 440.0] {
                let widths = panel_titlebar_widths(520.0, files, 1100.0, right_pad);
                let controls_left =
                    viewport - right_pad - widths.files_controls - widths.surface_reveal;
                assert_eq!(
                    controls_left,
                    viewport - files - 520.0,
                    "caption clearance {right_pad}, Files width {files}"
                );
                assert!(widths.files_controls >= PANEL_TOGGLE_SLOTS);
                if files >= right_pad + PANEL_TOGGLE_SLOTS {
                    assert_eq!(
                        viewport - right_pad - widths.files_controls,
                        viewport - files
                    );
                }
            }
        }
    }

    #[test]
    fn expanded_tabs_clear_the_left_controls_without_reserving_captions_twice() {
        for right_pad in [6.0, 92.0, 114.0] {
            let viewport = 1400.0;
            let files = 286.0;
            // With the sidebar open, the header starts exactly at its seam.
            // With it collapsed, leave room for the window/nav controls.
            for (sidebar, row_left) in [(256.0, 248.0), (0.0, 180.0)] {
                let widths = panel_titlebar_widths(
                    viewport - sidebar - files,
                    files,
                    viewport - row_left - right_pad - 8.0,
                    right_pad,
                );
                let controls_left =
                    viewport - right_pad - widths.files_controls - widths.surface_reveal;
                assert_eq!(controls_left, sidebar.max(row_left + 8.0));
            }
        }
    }

    #[test]
    fn narrow_panels_and_tight_headers_keep_nonnegative_reveal_widths() {
        // A narrow surface and Files share a 520px header.
        let widths = panel_titlebar_widths(234.0, 286.0, 600.0, 92.0);
        assert_eq!(
            1000.0 - 92.0 - widths.files_controls - widths.surface_reveal,
            480.0
        );
        for available in [-20.0, 0.0, 28.0, 56.0, 100.0] {
            let widths = panel_titlebar_widths(0.0, 0.0, available, 114.0);
            assert_eq!(widths.surface_reveal, 0.0);
            assert_eq!(widths.files_controls, PANEL_TOGGLE_SLOTS);
        }
    }
}

#[cfg(test)]
mod thread_conversation_tests {
    use super::*;

    fn chat(id: &str, minutes: i64, active_minutes: Option<i64>) -> zuse_protocol::Chat {
        let base = chrono::DateTime::parse_from_rfc3339("2026-10-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        serde_json::from_value(serde_json::json!({
            "id": id, "deviceId": "local", "title": null, "archived": false,
            "createdAt": base + chrono::Duration::minutes(minutes),
            "lastMessageAt": active_minutes.map(|m| base + chrono::Duration::minutes(m)),
            "threadId": "root",
        }))
        .unwrap()
    }

    fn ids(chats: &[zuse_protocol::Chat]) -> Vec<&str> {
        chats.iter().map(|chat| chat.id.as_str()).collect()
    }

    #[test]
    fn closed_conversations_move_to_history_most_recent_first() {
        let rows = vec![
            chat("b", 2, Some(5)),
            chat("root", 0, None),
            chat("c", 3, Some(90)),
            chat("d", 4, None),
        ];
        let closed = vec!["b".to_owned(), "c".to_owned()];
        let (open, hidden) = split_thread_conversations(rows, "root", &closed, "root");
        assert_eq!(ids(&open), ["root", "d"]);
        assert_eq!(ids(&hidden), ["c", "b"]);
    }

    #[test]
    fn the_selected_conversation_always_shows_and_the_strip_never_empties() {
        let rows = vec![chat("root", 0, None), chat("b", 1, None)];
        let closed = vec!["root".to_owned(), "b".to_owned()];
        let (open, hidden) = split_thread_conversations(rows.clone(), "root", &closed, "b");
        assert_eq!((ids(&open), ids(&hidden)), (vec!["b"], vec!["root"]));
        // Selection outside the thread (stale): the first conversation stays.
        let (open, hidden) = split_thread_conversations(rows, "root", &closed, "elsewhere");
        assert_eq!((ids(&open), ids(&hidden)), (vec!["root"], vec!["b"]));
    }

    #[test]
    fn closing_shows_the_right_neighbour_then_the_left() {
        let open = ["a", "b", "c"].map(String::from);
        assert_eq!(close_neighbour(&open, "b").as_deref(), Some("c"));
        assert_eq!(close_neighbour(&open, "c").as_deref(), Some("b"));
        assert_eq!(close_neighbour(&open, "a").as_deref(), Some("b"));
        assert_eq!(close_neighbour(&open[..1], "a"), None);
        assert_eq!(close_neighbour(&open, "gone"), None);
    }

    #[test]
    fn history_times_read_as_ago() {
        let now = Utc::now();
        assert_eq!(history_time(now, now), "now");
        assert_eq!(history_time(now - chrono::Duration::days(1), now), "1d ago");
    }
}

#[cfg(test)]
mod cycle_tests {
    use super::*;

    fn order(ids: &[&str]) -> Vec<String> {
        ids.iter().map(|id| id.to_string()).collect()
    }

    #[test]
    fn steps_forward_and_back_through_the_list() {
        let list = order(&["a", "b", "c"]);
        assert_eq!(cycle_target(&list, Some("a"), true).as_deref(), Some("b"));
        assert_eq!(cycle_target(&list, Some("b"), true).as_deref(), Some("c"));
        assert_eq!(cycle_target(&list, Some("c"), false).as_deref(), Some("b"));
        assert_eq!(cycle_target(&list, Some("b"), false).as_deref(), Some("a"));
    }

    #[test]
    fn wraps_at_both_ends() {
        let list = order(&["a", "b", "c"]);
        assert_eq!(cycle_target(&list, Some("c"), true).as_deref(), Some("a"));
        assert_eq!(cycle_target(&list, Some("a"), false).as_deref(), Some("c"));
    }

    #[test]
    fn a_single_session_cycles_to_itself() {
        // Not a no-op by accident: with one row both directions must resolve,
        // so the shortcut never looks broken by dead-ending on `None`.
        let list = order(&["only"]);
        assert_eq!(
            cycle_target(&list, Some("only"), true).as_deref(),
            Some("only")
        );
        assert_eq!(
            cycle_target(&list, Some("only"), false).as_deref(),
            Some("only")
        );
    }

    #[test]
    fn no_selection_enters_the_list_from_the_matching_end() {
        let list = order(&["a", "b", "c"]);
        assert_eq!(cycle_target(&list, None, true).as_deref(), Some("a"));
        assert_eq!(cycle_target(&list, None, false).as_deref(), Some("c"));
        assert_eq!(
            cycle_target(&list, Some("gone"), true).as_deref(),
            Some("a")
        );
        assert_eq!(
            cycle_target(&list, Some("gone"), false).as_deref(),
            Some("c")
        );
    }

    #[test]
    fn an_empty_list_has_nothing_to_select() {
        assert_eq!(cycle_target(&[], None, true), None);
        assert_eq!(cycle_target(&[], Some("a"), true), None);
    }

    // Cycling walks the rows the sidebar is drawing, not every chat: that
    // guarantee is structural now — `cycle_session` reads the same
    // `AppState::sidebar_chats` the sidebar and the jump shortcuts read, and
    // `jump_slots_count_the_rows_the_sidebar_draws` (state.rs) covers the
    // space-filter behaviour for all of them.
}

#[cfg(feature = "browser-fixture")]
impl Shell {
    /// Three conversations in the selected thread, the oldest sibling closed
    /// into the history menu. `hover` moves the pointer over the second tab
    /// so captures show its ×.
    pub(super) fn fixture_seed_thread_conversations(
        &mut self,
        hover: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(root) = self.state.read(cx).selected_chat.clone() else {
            return;
        };
        self.state.update(cx, |state, cx| {
            let Some(base) = state.chats.iter().find(|chat| chat.id == root).cloned() else {
                return;
            };
            for (id, title, harness, hours) in [
                (
                    "thread-review",
                    "Review the sync protocol",
                    zuse_protocol::HarnessId::Codex,
                    30,
                ),
                (
                    "thread-copy",
                    "Tidy onboarding copy",
                    zuse_protocol::HarnessId::ClaudeCode,
                    2,
                ),
                (
                    "thread-tests",
                    "Add snapshot tests",
                    zuse_protocol::HarnessId::Codex,
                    1,
                ),
            ] {
                let mut chat = base.clone();
                chat.id = id.into();
                chat.title = Some(title.into());
                chat.thread_id = Some(root.clone());
                chat.created_at = Utc::now() - chrono::Duration::hours(hours);
                chat.last_message_at = Some(chat.created_at);
                if let Some(config) = chat.config.as_mut() {
                    config.harness = harness;
                }
                state.chats.retain(|row| row.id != id);
                state.chats.push(chat);
            }
            cx.notify();
        });
        self.settings
            .closed_thread_conversations
            .insert(root, vec!["thread-review".into()]);
        if hover {
            // The real pointer, so the strip's own hover styling shows.
            cx.spawn(async move |_, cx| {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(300))
                    .await;
                let pid = std::process::id().to_string();
                if let Ok(out) = std::process::Command::new("xdotool")
                    .args(["search", "--onlyvisible", "--pid", &pid])
                    .output()
                    && let Some(window) = String::from_utf8_lossy(&out.stdout).lines().next()
                {
                    let (x, y) = std::env::var("ZUSE_FIXTURE_HOVER")
                        .ok()
                        .and_then(|at| {
                            let (x, y) = at.split_once(',')?;
                            Some((x.to_owned(), y.to_owned()))
                        })
                        .unwrap_or_else(|| ("520".into(), "24".into()));
                    let _ = std::process::Command::new("xdotool")
                        .args(["mousemove", "--window", window, &x, &y])
                        .status();
                }
            })
            .detach();
        }
        cx.notify();
    }
}
