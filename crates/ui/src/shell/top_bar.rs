//! Workspace overview navigation, conversation tabs, and checkout controls.
use super::*;
use crate::localization::message as tr;
use zuse_i18n::messages as msg;

#[cfg(test)]
#[path = "top_bar_tests.rs"]
mod tests;

/// The conversation tab row under the top bar.
pub(super) const CONVERSATION_TAB_ROW_HEIGHT: f32 = 36.0;
const OVERVIEW_TAB_WIDTH: f32 = 160.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum OverviewTab {
    Threads,
    PullRequests,
    Plugins,
}

impl OverviewTab {
    const ALL: [Self; 3] = [Self::Threads, Self::PullRequests, Self::Plugins];

    pub(super) fn from_route(route: Route) -> Option<Self> {
        match route {
            Route::Chat => Some(Self::Threads),
            Route::PullRequests => Some(Self::PullRequests),
            Route::Plugins => Some(Self::Plugins),
            _ => None,
        }
    }

    fn id(self) -> &'static str {
        match self {
            Self::Threads => "overview-threads",
            Self::PullRequests => "overview-pull-requests",
            Self::Plugins => "overview-plugins",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Threads => "Threads",
            Self::PullRequests => "Pull requests",
            Self::Plugins => "Plugins",
        }
    }

    fn glyph(self) -> &'static str {
        match self {
            Self::Threads => icons::DITHER_CHAT,
            Self::PullRequests => icons::PULL_REQUEST,
            Self::Plugins => icons::DITHER_PLUG,
        }
    }
}

#[derive(Clone)]
pub(super) struct OverviewEntry {
    id: u64,
    kind: OverviewTab,
    selected_chat: Option<String>,
    cloud_title: Option<String>,
    pull_requests_page: Option<Entity<PullRequestsPage>>,
}

impl OverviewEntry {
    pub(super) fn new(id: u64, kind: OverviewTab) -> Self {
        Self {
            id,
            kind,
            selected_chat: None,
            cloud_title: None,
            pull_requests_page: None,
        }
    }

    pub(super) fn clear_pull_requests_page(&mut self) {
        self.pull_requests_page = None;
    }
}

/// The branch menu's state: the checkout's local branches, fetched when the
/// menu opens, and a switch in flight.
#[derive(Default)]
pub(super) struct BranchMenu {
    pub(super) menu: popover::Popup<()>,
    pub(super) branches: Vec<String>,
    loaded_for: Option<String>,
    current_branch: Option<String>,
    loading: bool,
    error: Option<SharedString>,
    switching: Option<String>,
    task: Option<Task<()>>,
}

/// Where the selected conversation's branch lives, for the branch RPCs.
struct BranchTarget {
    chat_id: String,
    branch: String,
    repo_path: Option<String>,
    target_device: Option<String>,
    dirty: u32,
}

fn format_branch_error(branch: &str, error: &str) -> String {
    if error.contains("already used by worktree") || error.contains("already checked out at") {
        let path = error.rsplit('\'').nth(1).unwrap_or("");
        let location = if path.is_empty() {
            String::new()
        } else {
            format!("\n{path}")
        };
        format!(
            "Branch already in use\n“{branch}” is checked out in another worktree.{location}\nSwitch that worktree to another branch first."
        )
    } else if error.contains("local changes") || error.contains("would be overwritten") {
        format!(
            "Uncommitted changes\nCommit or stash your changes before switching branches.\n{error}"
        )
    } else {
        format!("Couldn’t switch branch\n{error}")
    }
}

pub(super) struct ComposerBranch {
    shell: gpui::WeakEntity<Shell>,
    _observation: Subscription,
}

impl ComposerBranch {
    pub(super) fn new(shell: Entity<Shell>, cx: &mut Context<Self>) -> Self {
        Self {
            shell: shell.downgrade(),
            _observation: cx.observe(&shell, |_, _, cx| cx.notify()),
        }
    }
}

impl Render for ComposerBranch {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.shell
            .update(cx, |shell, cx| {
                let theme = Theme::of(cx).clone();
                shell
                    .render_top_bar_branch(&theme, cx)
                    .unwrap_or_else(|| Empty.into_any_element())
            })
            .unwrap_or_else(|_| Empty.into_any_element())
    }
}

impl Shell {
    fn overview_label(&self, entry: &OverviewEntry, cx: &App) -> SharedString {
        if entry.kind != OverviewTab::Threads {
            return entry.kind.label().into();
        }
        let active = entry.id == self.active_overview_tab && matches!(self.route, Route::Chat);
        let cloud_title = if active {
            self.cloud_conversation
                .as_ref()
                .map(|view| view.read(cx).title.as_str().to_owned())
        } else {
            entry.cloud_title.clone()
        };
        if let Some(title) = cloud_title.filter(|title| !title.trim().is_empty()) {
            return transcript::single_line(&title).into();
        }
        let state = self.state.read(cx);
        let chat_id = if active {
            state.selected_chat.as_deref()
        } else {
            entry.selected_chat.as_deref()
        };
        let Some(chat_id) = chat_id else {
            return "New threads".into();
        };
        let chat = state.chats.iter().find(|chat| chat.id == chat_id);
        let thread = chat
            .and_then(|chat| state.chats.iter().find(|root| root.id == chat.thread_id()))
            .or(chat);
        let title = thread
            .and_then(|chat| {
                chat.title
                    .as_deref()
                    .filter(|title| !title.trim().is_empty())
                    .or_else(|| {
                        chat.last_message_preview
                            .as_deref()
                            .filter(|preview| !preview.trim().is_empty())
                    })
            })
            .unwrap_or("Untitled thread");
        transcript::single_line(title).into()
    }

    fn save_overview_state(&mut self, cx: &App) {
        if let Some(entry) = self
            .overview_tabs
            .iter_mut()
            .find(|entry| entry.id == self.active_overview_tab)
        {
            if entry.kind == OverviewTab::Threads {
                entry.selected_chat = self.state.read(cx).selected_chat.clone();
                entry.cloud_title = self
                    .cloud_conversation
                    .as_ref()
                    .map(|view| view.read(cx).title.clone());
            } else if entry.kind == OverviewTab::PullRequests {
                entry.pull_requests_page = self.pull_requests_page.clone();
            }
        }
    }

    pub(super) fn sync_overview_route(&mut self, route: Route, cx: &mut Context<Self>) {
        let Some(kind) = OverviewTab::from_route(route) else {
            return;
        };
        if self
            .overview_tabs
            .iter()
            .any(|entry| entry.id == self.active_overview_tab && entry.kind == kind)
        {
            return;
        }
        self.save_overview_state(cx);
        let id = self
            .overview_tabs
            .iter()
            .find(|entry| entry.kind == kind)
            .map(|entry| entry.id)
            .unwrap_or_else(|| self.insert_overview_tab(kind));
        self.active_overview_tab = id;
        if kind == OverviewTab::PullRequests {
            self.pull_requests_page = self
                .overview_tabs
                .iter()
                .find(|entry| entry.id == id)
                .and_then(|entry| entry.pull_requests_page.clone());
        }
    }

    fn insert_overview_tab(&mut self, kind: OverviewTab) -> u64 {
        let id = self.next_overview_tab;
        self.next_overview_tab += 1;
        self.overview_tabs.push(OverviewEntry::new(id, kind));
        id
    }

    fn add_overview_tab(&mut self, kind: OverviewTab, cx: &mut Context<Self>) {
        let id = self.insert_overview_tab(kind);
        self.activate_overview_tab(id, cx);
        if kind == OverviewTab::Threads {
            self.open_new_session(None, cx);
        }
    }

    fn activate_overview_tab(&mut self, id: u64, cx: &mut Context<Self>) {
        self.close_overview_menu(cx);
        let Some(entry) = self
            .overview_tabs
            .iter()
            .find(|entry| entry.id == id)
            .cloned()
        else {
            return;
        };
        if self.active_overview_tab == id && OverviewTab::from_route(self.route) == Some(entry.kind)
        {
            cx.notify();
            return;
        }
        self.save_overview_state(cx);
        self.active_overview_tab = id;
        if let Some(index) = self.overview_tabs.iter().position(|entry| entry.id == id) {
            self.overview_scroll.scroll_to_item(index);
        }
        match entry.kind {
            OverviewTab::Threads => {
                let nav_entry = NavEntry::Chat(entry.selected_chat.clone().unwrap_or_default());
                self.set_route(Route::Chat, cx);
                if self.state.read(cx).selected_chat != entry.selected_chat {
                    self.state
                        .update(cx, |state, cx| state.select_chat(entry.selected_chat, cx));
                }
                if self.nav.current() != &nav_entry {
                    self.nav.push(nav_entry);
                }
            }
            OverviewTab::PullRequests => {
                let already_visible = matches!(self.route, Route::PullRequests);
                if already_visible {
                    if let Some(page) = self.pull_requests_page.clone() {
                        page.update(cx, |page, _| page.on_hidden());
                    }
                }
                self.pull_requests_page = entry.pull_requests_page;
                self.open_pull_requests(cx);
                if already_visible {
                    if let Some(page) = self.pull_requests_page.clone() {
                        page.update(cx, |page, cx| page.on_visible(cx));
                    }
                }
            }
            OverviewTab::Plugins => self.open_plugins_page(cx),
        }
        cx.notify();
    }

    fn close_overview_menu(&mut self, cx: &mut Context<Self>) {
        if self.overview_add_menu.begin_close() {
            popover::reap_popup(cx, |shell: &mut Self| &mut shell.overview_add_menu);
        }
    }

    fn close_overview_tab(&mut self, id: u64, cx: &mut Context<Self>) {
        if self.overview_tabs.len() <= 1 {
            return;
        }
        let Some(index) = self.overview_tabs.iter().position(|entry| entry.id == id) else {
            return;
        };
        self.overview_tabs.remove(index);
        if self.active_overview_tab == id {
            let next = self.overview_tabs[index.saturating_sub(1)].id;
            self.activate_overview_tab(next, cx);
        }
        cx.notify();
    }

    pub(super) fn render_overview_tabs(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = Theme::of(cx).clone();
        let mut tabs = div()
            .id("overview-tab-scroll")
            .flex()
            .items_center()
            .min_w_0()
            .overflow_x_scroll()
            .track_scroll(&self.overview_scroll)
            .gap(px(4.0));
        let row = div()
            .id("overview-tabs")
            .flex()
            .items_center()
            .gap(px(4.0))
            .min_w_0();
        let closable = self.overview_tabs.len() > 1;
        for (index, entry) in self.overview_tabs.clone().into_iter().enumerate() {
            let tab = entry.kind;
            let id = entry.id;
            let label = self.overview_label(&entry, cx);
            let selected =
                self.active_overview_tab == id && OverviewTab::from_route(self.route) == Some(tab);
            let tone = if selected {
                theme.text
            } else {
                theme.text_muted
            };
            let mut item = div()
                .id(("overview-tab", id as usize))
                .debug_selector({
                    let first = !self.overview_tabs[..index]
                        .iter()
                        .any(|entry| entry.kind == tab);
                    move || {
                        if first {
                            tab.id().into()
                        } else {
                            format!("overview-tab-{id}")
                        }
                    }
                })
                .role(gpui::Role::Button)
                .aria_label(label.clone())
                .aria_selected(selected)
                .h(px(28.0))
                .w(px(OVERVIEW_TAB_WIDTH))
                .min_w_0()
                .flex_none()
                .px(px(9.0))
                .rounded(px(6.0))
                .border_1()
                .border_color(if selected {
                    theme.border_strong.opacity(0.8)
                } else {
                    theme.border.opacity(0.0)
                })
                .flex()
                .items_center()
                .gap(px(7.0))
                .cursor_pointer()
                .text_size(crate::typography::text_sm())
                .text_color(tone)
                .bg(if selected {
                    theme.surface_card
                } else {
                    crate::theme::wash(0.0)
                })
                .hover(|style| style.bg(theme.element_hover))
                .child(icon(tab.glyph()).size(px(15.0)).text_color(tone))
                .child(div().flex_1().min_w_0().truncate().child(label.clone()))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.activate_overview_tab(id, cx);
                }));
            if closable {
                item = item.child(
                    div()
                        .id(("close-overview", id as usize))
                        .role(gpui::Role::Button)
                        .aria_label(format!("Close {label}"))
                        .size(px(18.0))
                        .flex_none()
                        .rounded(px(4.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .hover(|style| style.bg(crate::theme::wash(0.10)))
                        .child(
                            icon(icons::CLOSE)
                                .size(px(12.0))
                                .text_color(theme.text_muted),
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.close_overview_tab(id, cx);
                        })),
                );
            }
            tabs = tabs.child(item);
        }
        let mut add = header_icon_button_base(
            "overview-add",
            icon(icons::PLUS)
                .size(px(14.0))
                .text_color(theme.text_muted),
            cx.listener(|this, _, _, cx| {
                if this.overview_add_menu.take_press_was_open() {
                    this.close_overview_menu(cx);
                } else {
                    this.overview_add_menu.open(());
                }
                cx.notify();
            }),
        )
        .when(self.overview_add_menu.get().is_none(), |button| {
            button.tip("Open overview tab")
        })
        .debug_selector(|| "overview-add".into())
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _, window, _| {
                window.prevent_default();
                this.overview_add_menu.note_trigger_press();
            }),
        )
        .relative();
        if self.overview_add_menu.get().is_some() {
            let mut menu = popover::popover_card(&theme.for_popup())
                .w(px(200.0))
                .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                    this.close_overview_menu(cx);
                    cx.notify();
                }));
            for tab in OverviewTab::ALL {
                menu = menu.child(
                    popover::menu_row(&theme.for_popup(), false, format!("add-{}", tab.id()))
                        .id(("add-overview", tab as usize))
                        .debug_selector(move || format!("add-{}", tab.id()))
                        .child(
                            icon(tab.glyph())
                                .size(px(15.0))
                                .text_color(theme.text_muted),
                        )
                        .child(tab.label())
                        .on_click(
                            cx.listener(move |this, _, _, cx| this.add_overview_tab(tab, cx)),
                        ),
                );
            }
            add = add.child(popover::anchored_menu_below(
                "overview-add-menu",
                menu.into_any_element(),
                self.overview_add_menu.closing_since(),
            ));
        }
        row.child(tabs).child(add).into_any_element()
    }

    fn branch_target(&self, cx: &App) -> Option<BranchTarget> {
        let conversation = self.conversation_state(cx);
        let state = conversation.read(cx);
        let chat = state.selected_chat_row()?;
        let mut branch =
            crate::change_requests::conversation_branch(chat, &state.spaces)?.to_owned();
        let repo_path = chat
            .source_context
            .as_ref()
            .map(|source| source.repo_root.clone())
            .or_else(|| chat.cwd.clone());
        let target_device = (self.cloud_conversation.is_none()
            && state.local_device_id.as_deref() != Some(chat.device_id.as_str()))
        .then(|| chat.device_id.clone());
        if let Some(path) = &repo_path {
            let key = format!("{}:{path}", target_device.as_deref().unwrap_or("local"));
            if self.branch_menu.loaded_for.as_deref() == Some(key.as_str()) {
                if let Some(current) = &self.branch_menu.current_branch {
                    branch = current.clone();
                }
            }
        }
        Some(BranchTarget {
            chat_id: chat.id.clone(),
            branch,
            repo_path,
            target_device,
            dirty: state.dirty_files_for_chat(chat),
        })
    }

    /// Whether the conversation tab row shows: on a selected conversation.
    pub(super) fn conversation_tab_row_visible(&self, cx: &App) -> bool {
        matches!(self.route, Route::Chat)
            && self.has_conversation(cx)
            && !self.state.read(cx).selected_chat_is_zot_coordinator()
    }

    /// The conversation tabs on their own row below the top bar: main-area
    /// tabs, the thread's conversations and side chats, `+`, and history.
    pub(super) fn render_conversation_tab_row(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = Theme::of(cx).clone();
        let tabs = if self.cloud_conversation.is_some() {
            self.render_cloud_tabs(cx)
                .unwrap_or_else(|| Empty.into_any_element())
        } else {
            self.render_thread_conversations(cx)
        };
        div()
            .id("conversation-tab-row")
            .debug_selector(|| "conversation-tab-row".into())
            .absolute()
            .top(px(Theme::TITLEBAR_HEIGHT))
            .left_0()
            .right_0()
            .h(px(CONVERSATION_TAB_ROW_HEIGHT))
            .pl(px((self.overview_content_left(cx)
                - self.sidebar_now()
                - self.workspace_gutter())
            .max(Theme::SPACE_SM)))
            .pr(px(Theme::SPACE_SM))
            .flex()
            .items_center()
            .text_size(crate::typography::text_xs())
            .text_color(theme.text)
            // The transcript fades out beneath this row; a wheel over the
            // tabs scrolls the strip, never the conversation behind it.
            .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
            .occlude()
            .child(tabs)
            .into_any_element()
    }

    /// The checkout pill and its branch menu, absent without a checkout.
    pub(super) fn render_top_bar_branch(
        &mut self,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let Some(target) = self.branch_target(cx) else {
            let branch = self.cloud_conversation.as_ref()?.read(cx).branch_label()?;
            // Saved task metadata can identify the branch before the runtime
            // supplies a checkout context. Keep it read-only until then.
            return Some(
                div()
                    .debug_selector(|| "cloud-saved-branch".into())
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .text_color(theme.text_muted)
                    .child(icon(icons::GIT_BRANCH).size(px(14.)))
                    .child(div().max_w(px(180.)).truncate().child(branch))
                    .into_any_element(),
            );
        };
        let open = self.branch_menu.menu.get().is_some();
        let label: SharedString = target.branch.clone().into();
        let fade = "top-bar-branch";
        let mut trigger = div()
            .id("top-bar-branch")
            .debug_selector(|| "top-bar-branch".into())
            .role(gpui::Role::Button)
            .aria_label(tr(cx, msg::chat::top_bar_switch_branch))
            .aria_expanded(open)
            .h(px(24.0))
            .min_w_0()
            .flex_1()
            .flex()
            .items_center()
            .gap(px(6.0))
            .px(px(9.0))
            .rounded_full()
            .border_1()
            .border_color(theme.border)
            .cursor_pointer()
            .text_color(if open { theme.text } else { theme.text_muted })
            .bg(theme.surface_card.opacity(1.0))
            .hover(|style| style.bg(theme.element_hover))
            .on_hover(motion::hover_listener(fade))
            .occlude()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, _| {
                    window.prevent_default();
                    this.branch_menu.menu.note_trigger_press();
                }),
            )
            .on_click(cx.listener(|this, _, _, cx| {
                cx.stop_propagation();
                if this.branch_menu.menu.take_press_was_open() {
                    this.close_branch_menu(cx);
                } else {
                    this.open_branch_menu(cx);
                }
            }))
            .tip(label.clone())
            .tooltip_show_delay(Duration::from_millis(350))
            .child(
                icon(icons::GIT_BRANCH)
                    .size(px(13.0))
                    .text_color(theme.text_muted),
            )
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .max_w(px(180.0))
                    .truncate()
                    .child(label),
            )
            .when(target.dirty > 0, |d| {
                d.child(
                    div()
                        .flex_none()
                        .text_color(theme.text_muted)
                        .child(format!("· {}", target.dirty)),
                )
            })
            .child(
                icon(icons::ALT_ARROW_DOWN)
                    .size(px(12.0))
                    .flex_none()
                    .text_color(theme.text_muted),
            );
        if open {
            let closing = self.branch_menu.menu.closing_since();
            let menu = self.render_branch_menu(&target, theme, cx);
            let popup = if self.cloud_conversation.is_some() {
                popover::anchored_menu_below("top-bar-branch-menu", menu, closing)
            } else {
                popover::anchored_menu_above_end("top-bar-branch-menu", menu, closing)
            };
            trigger = trigger.relative().child(popup);
        }
        Some(
            div()
                .id("top-bar-identity")
                .debug_selector(|| "top-bar-identity".into())
                .role(gpui::Role::Navigation)
                .aria_label(tr(cx, msg::projects::pr_pane_branch))
                .min_w_0()
                .w_full()
                .flex()
                .items_center()
                .gap(px(4.0))
                .text_size(px(12.0))
                .child(trigger)
                .into_any_element(),
        )
    }

    fn copy_branch_name(&mut self, branch: String, cx: &mut Context<Self>) {
        self.close_branch_menu(cx);
        cx.write_to_clipboard(ClipboardItem::new_string(branch));
        self.show_notice(crate::toast::ToastKind::Success, "Branch name copied", cx);
    }

    fn open_branch_menu(&mut self, cx: &mut Context<Self>) {
        self.branch_menu.menu.open(());
        self.branch_menu.error = None;
        self.refresh_branch_list(cx);
        cx.notify();
    }

    pub(super) fn close_branch_menu(&mut self, cx: &mut Context<Self>) {
        if self.branch_menu.menu.begin_close() {
            popover::reap_popup(cx, |shell: &mut Self| &mut shell.branch_menu.menu);
        }
        cx.notify();
    }

    /// Fetch the checkout's branches. The list shows what it had while the
    /// request runs; a different checkout starts from empty.
    fn refresh_branch_list(&mut self, cx: &mut Context<Self>) {
        let Some(target) = self.branch_target(cx) else {
            return;
        };
        let Some(repo_path) = target.repo_path.clone() else {
            return;
        };
        let Some(engine) = self.conversation_state(cx).read(cx).engine().cloned() else {
            return;
        };
        let key = format!(
            "{}:{repo_path}",
            target.target_device.as_deref().unwrap_or("local")
        );
        if self.branch_menu.loaded_for.as_deref() != Some(key.as_str()) {
            self.branch_menu.branches.clear();
            self.branch_menu.current_branch = None;
        }
        self.branch_menu.loaded_for = Some(key.clone());
        self.branch_menu.loading = true;
        let mut params = serde_json::Map::new();
        params.insert("repoPath".into(), repo_path.into());
        params.insert("chatId".into(), target.chat_id.into());
        if let Some(device) = target.target_device {
            params.insert("targetDeviceId".into(), device.into());
        }
        self.branch_menu.task = Some(cx.spawn(async move |this, cx| {
            let refs_params = serde_json::Value::Object(params.clone());
            let result = engine
                .client()
                .call(methods::LIST_BRANCHES, serde_json::Value::Object(params))
                .await;
            let refs = engine.client().call(methods::LIST_REFS, refs_params).await;
            let _ = this.update(cx, |this, cx| {
                if this.branch_menu.loaded_for.as_deref() != Some(key.as_str()) {
                    return;
                }
                this.branch_menu.loading = false;
                if let Ok(value) = refs {
                    if let Ok(refs) = serde_json::from_value::<Vec<zuse_protocol::RepoRef>>(value) {
                        this.branch_menu.current_branch = refs
                            .into_iter()
                            .find(|item| item.current)
                            .map(|item| item.name);
                    }
                }
                match result {
                    Ok(value) => {
                        this.branch_menu.branches =
                            serde_json::from_value::<Vec<String>>(value).unwrap_or_default();
                    }
                    Err(error) => this.branch_menu.error = Some(error.to_string().into()),
                }
                cx.notify();
            });
        }));
    }

    /// Check out `branch` in the conversation's checkout. Git refuses when
    /// uncommitted changes conflict; its message shows in the menu.
    fn switch_top_bar_branch(&mut self, branch: String, cx: &mut Context<Self>) {
        if self.branch_menu.switching.is_some() {
            return;
        }
        let Some(target) = self.branch_target(cx) else {
            return;
        };
        if target.branch == branch {
            self.close_branch_menu(cx);
            return;
        }
        let (Some(repo_path), Some(engine)) = (
            target.repo_path,
            self.conversation_state(cx).read(cx).engine().cloned(),
        ) else {
            return;
        };
        let mut params = serde_json::Map::new();
        let key = format!(
            "{}:{repo_path}",
            target.target_device.as_deref().unwrap_or("local")
        );
        params.insert("repoPath".into(), repo_path.into());
        params.insert("refName".into(), branch.clone().into());
        if let Some(device) = target.target_device {
            params.insert("targetDeviceId".into(), device.into());
        }
        self.branch_menu.error = None;
        self.branch_menu.switching = Some(branch.clone());
        self.branch_menu.task = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(methods::SWITCH_REF, serde_json::Value::Object(params))
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.branch_menu.loaded_for.as_deref() != Some(key.as_str()) {
                    return;
                }
                this.branch_menu.switching = None;
                match result {
                    Ok(value) => {
                        this.branch_menu.current_branch = Some(
                            value
                                .get("branch")
                                .and_then(|value| value.as_str())
                                .unwrap_or(&branch)
                                .to_owned(),
                        );
                        this.close_branch_menu(cx)
                    }
                    Err(error) => {
                        this.branch_menu.error =
                            Some(format_branch_error(&branch, &error.to_string()).into())
                    }
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

    /// The branch menu: the current branch with a copy button, the
    /// checkout's local branches (current one checked), and copy.
    fn render_branch_menu(
        &mut self,
        target: &BranchTarget,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = theme.for_popup();
        let current = target.branch.clone();
        let section_label = |label: &'static str| {
            div()
                .px(px(8.0))
                .pt(px(4.0))
                .pb(px(2.0))
                .text_size(crate::typography::text_xs())
                .text_color(theme.text_muted)
                .child(label)
        };
        let loading = self.branch_menu.loading && self.branch_menu.branches.is_empty();
        let switching = self.branch_menu.switching.clone();
        let rows = self
            .branch_menu
            .branches
            .iter()
            .map(|name| {
                let is_current = *name == current;
                let fade = format!("branch-menu-row-{name}");
                let pick = name.clone();
                let busy = switching.as_deref() == Some(name.as_str());
                popover::menu_row(&theme, is_current, fade.clone())
                    .id(SharedString::from(fade))
                    .role(gpui::Role::MenuItem)
                    .aria_label(SharedString::from(name.clone()))
                    .aria_selected(is_current)
                    .gap(px(8.0))
                    .h(px(28.0))
                    .py(px(3.0))
                    .text_size(px(12.0))
                    .child(
                        icon(icons::CHECK)
                            .size(px(14.0))
                            .flex_none()
                            .text_color(theme.text)
                            .when(!is_current, |glyph| glyph.opacity(0.0)),
                    )
                    .child(div().flex_1().min_w_0().truncate().child(name.clone()))
                    .when(busy, |row| {
                        row.child(
                            div()
                                .flex_none()
                                .text_color(theme.text_muted)
                                .child(tr(cx, msg::native::switching)),
                        )
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.switch_top_bar_branch(pick.clone(), cx)
                    }))
                    .into_any_element()
            })
            .collect::<Vec<_>>();
        let empty = rows.is_empty();
        let copy_branch = current.clone();
        let longest = self
            .branch_menu
            .branches
            .iter()
            .map(|name| name.chars().count())
            .chain(std::iter::once(current.chars().count()))
            .max()
            .unwrap_or(0);
        let width = (longest as f32 * 6.0 + 48.0).clamp(224.0, 288.0);
        popover::popover_card(&theme)
            .w(px(width))
            .bg(theme.surface_card.opacity(1.0))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| this.close_branch_menu(cx)))
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .pl(px(8.0))
                    .py(px(2.0))
                    .child(
                        icon(icons::GIT_BRANCH)
                            .size(px(14.0))
                            .text_color(theme.text_muted),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(crate::typography::text_xs())
                            .text_color(theme.text)
                            .child(current.clone()),
                    )
                    .child(header_icon_button(
                        "branch-menu-copy",
                        icons::COPY,
                        "Copy branch name",
                        &theme,
                        cx.listener(move |this, _, _, cx| {
                            this.copy_branch_name(copy_branch.clone(), cx)
                        }),
                    )),
            )
            .child(popover::menu_separator())
            .when_some(self.branch_menu.error.clone(), |card, error| {
                let (title, detail) = error
                    .split_once('\n')
                    .map(|(title, detail)| (title.to_owned(), detail.to_owned()))
                    .unwrap_or_else(|| ("Couldn’t load branches".into(), error.to_string()));
                card.child(
                    div()
                        .px(px(8.0))
                        .py(px(8.0))
                        .rounded(px(6.0))
                        .bg(theme.danger.opacity(0.08))
                        .flex()
                        .flex_col()
                        .gap(px(4.0))
                        .text_size(crate::typography::text_xs())
                        .child(
                            div()
                                .font_weight(gpui::FontWeight::MEDIUM)
                                .text_color(theme.danger)
                                .child(title),
                        )
                        .child(div().text_color(theme.text).child(detail)),
                )
            })
            .child(section_label("Local branches"))
            .child(
                div()
                    .id("branch-menu-list")
                    .max_h(px(224.0))
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .children(rows)
                    .when(empty, |list| {
                        list.child(
                            div()
                                .px(px(8.0))
                                .py(px(6.0))
                                .text_size(crate::typography::text_xs())
                                .text_color(theme.text_muted)
                                .child(if loading {
                                    ("Loading branches…").to_string()
                                } else {
                                    crate::localization::text("chat-top_bar_no_local_branches", cx)
                                }),
                        )
                    }),
            )
            .when(target.dirty > 0, |card| {
                card.child(popover::menu_separator()).child(
                    div()
                        .px(px(8.0))
                        .py(px(4.0))
                        .text_size(crate::typography::text_xs())
                        .text_color(theme.text_muted)
                        .child(format!(
                            "{} uncommitted {}",
                            target.dirty,
                            if target.dirty == 1 {
                                "change"
                            } else {
                                "changes"
                            }
                        )),
                )
            })
            .into_any_element()
    }
}

#[cfg(feature = "browser-fixture")]
impl Shell {
    /// Open the branch menu over a fixed branch list (no engine to ask).
    pub(super) fn fixture_open_branch_menu(&mut self, cx: &mut Context<Self>) {
        self.branch_menu.branches = [
            "main",
            "alex/connection-handoff",
            "alex/file-tree-icons",
            "release/0.9",
        ]
        .map(str::to_owned)
        .to_vec();
        self.branch_menu.menu.open(());
        cx.notify();
    }
}
