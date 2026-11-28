//! Coordinator chrome; commands use the same engine authority as external clients.
use super::zapper_forms::{Form, Kind, ZapperAfter};
use super::*;
use zuse_protocol::presentation::zapper_panel::{
    NeedsYouItem, NeedsYouTarget, ThreadRank, ZAPPER_SECTIONS, ZapperPage, ZapperPanelTabs,
    ZapperTab, finished_zapper_threads, needs_you_items, ordered_zapper_threads,
};
use zuse_protocol::zappers::*;

#[derive(Default)]
pub(super) struct ZapperUi {
    /// New Zapper or Zapper settings. Outlives the route so Back returns to the draft.
    pub(super) page: Option<Form>,
    pub(super) dialog: Option<Form>,
    pub(super) panel_hidden: bool,
    tabs: ZapperPanelTabs,
    /// The Zapper `tabs` belong to; another Zapper starts on a bare Overview.
    tabs_zapper: Option<String>,
    /// The strip's `…` menu.
    overflow: popover::Popup<()>,
    /// Scroll of the open tabs, so a newly shown tab is brought into view.
    open_scroll: gpui::ScrollHandle,
    /// Parsed Drive documents by path, with the version they were parsed at.
    documents: std::collections::HashMap<String, (u32, std::sync::Arc<crate::markdown::BlockTree>)>,
    pub(super) issues: Option<Entity<zuse_viewport::zapper_issues::IssuePanel>>,
    /// The issue surface's selection when it last notified. Only a change
    /// in it is the surface opening an issue; its other notifications (a
    /// hover, a sync, the click that opened a pull request) leave the active
    /// tab alone, since the surface keeps its last issue while another tab
    /// shows.
    issue_followed: Option<String>,
    pub(super) pull_request_page: Option<(
        String,
        Entity<crate::pull_request_detail::PullRequestDetailPage>,
    )>,
    pull_request_subscription: Option<Subscription>,
    files: Vec<ZapperFile>,
    /// The Zapper `files` came from, kept while a reload is in flight.
    files_zapper: Option<String>,
    /// Drive's open folders and selected row, for the Zapper in `tree_zapper`.
    tree_open: std::collections::BTreeSet<String>,
    tree_selected: Option<String>,
    tree_zapper: Option<String>,
    /// The created folders of the Zapper Drive last showed.
    tree_folders: Vec<String>,
    /// Keyboard focus of the file tree: arrows move, Enter opens, F2 moves,
    /// Delete deletes.
    tree_focus: Option<FocusHandle>,
    /// Threads just archived here. The engine applies it on its next pass, a
    /// few seconds later; until then the row should already be gone.
    archiving: std::collections::HashMap<String, std::time::Instant>,
    /// The Threads tab's "Finished" group (released threads) is expanded.
    finished_open: bool,
    /// Right-click menu of a sidebar Zapper row: its id and where it opened.
    pub(super) menu: popover::Popup<(String, Point<Pixels>)>,
    /// The menu's Slack submenu (linking the coordinator chat) is open.
    pub(super) slack_open: bool,
    pub(super) submenu: super::slack_link::Submenu,
    /// The Zapper waiting on "Delete Zapper?" confirmation.
    pub(super) delete_confirm: Option<String>,
    pub(super) loaded: Option<(String, u32, u32)>,
    events: Vec<ZapperEvent>,
    loading: bool,
    pub(super) pending: bool,
    pub(super) retry: Option<ZapperCommand>,
    /// Set while the issue panel hands a command to `send_zapper_command`, so
    /// only its own results close or fail its editor.
    pub(super) issue_command: bool,
    confirm_delete: bool,
}
fn action(
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
    theme: &Theme,
    callback: impl Fn(&mut Shell, &gpui::ClickEvent, &mut Window, &mut Context<Shell>) + 'static,
    cx: &mut Context<Shell>,
) -> AnyElement {
    div()
        .id(id.into())
        .px(px(8.0))
        .py(px(5.0))
        .rounded(px(4.0))
        .cursor_pointer()
        .text_size(crate::typography::ui_rems(12.0))
        .text_color(theme.text)
        .hover(|s| s.bg(theme.accent.opacity(0.12)))
        .child(label.into())
        .on_click(cx.listener(callback))
        .into_any_element()
}

fn section(text: impl Into<SharedString>, theme: &Theme) -> gpui::Div {
    div()
        .pb(px(4.0))
        .text_size(crate::typography::ui_rems(11.0))
        .text_color(theme.text_faint)
        .child(text.into())
}
fn muted(text: impl Into<SharedString>, theme: &Theme) -> gpui::Div {
    div()
        .py(px(4.0))
        .text_size(crate::typography::ui_rems(12.0))
        .text_color(theme.text_faint)
        .child(text.into())
}
#[allow(clippy::too_many_arguments)]
fn list_row(
    id: impl Into<SharedString>,
    glyph: &'static str,
    tint: gpui::Hsla,
    title: String,
    trailing: Option<String>,
    theme: &Theme,
    callback: impl Fn(&mut Shell, &gpui::ClickEvent, &mut Window, &mut Context<Shell>) + 'static,
    cx: &mut Context<Shell>,
) -> AnyElement {
    div()
        .id(id.into())
        .role(gpui::Role::Button)
        .aria_label(SharedString::from(title.clone()))
        .mx(px(-8.0))
        .flex()
        .items_center()
        .gap(px(8.0))
        .px(px(8.0))
        .py(px(5.0))
        .rounded(px(6.0))
        .cursor_pointer()
        .text_size(crate::typography::ui_rems(12.5))
        .text_color(theme.text)
        .hover(|d| d.bg(theme.element_hover))
        .child(icon(glyph).size(px(13.0)).flex_none().text_color(tint))
        .child(div().flex_1().min_w_0().truncate().child(title))
        .when_some(trailing, |d, t| {
            d.child(
                div()
                    .text_size(crate::typography::ui_rems(11.0))
                    .text_color(theme.text_faint)
                    .child(t),
            )
        })
        .on_click(cx.listener(callback))
        .into_any_element()
}

fn thread_action(
    id: String,
    glyph: &'static str,
    label: &'static str,
    theme: &Theme,
    callback: impl Fn(&mut Shell, &gpui::ClickEvent, &mut Window, &mut Context<Shell>) + 'static,
    cx: &mut Context<Shell>,
) -> AnyElement {
    // GPUI paints an SVG with the icon's own text colour, never its
    // parent's, so the colour and its hover state sit on the icon itself.
    let group = SharedString::from(format!("{id}-action"));
    div()
        .id(SharedString::from(id))
        .group(group.clone())
        .role(gpui::Role::Button)
        .aria_label(label)
        .size(px(24.0))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(6.0))
        .bg(gpui::rgb(0x242424))
        .cursor_pointer()
        .tip(label)
        .child(
            icon(glyph)
                .size(px(13.0))
                .text_color(theme.text_muted)
                .group_hover(group, |s| s.text_color(theme.text_strong)),
        )
        .on_click(cx.listener(move |this, event, window, cx| {
            cx.stop_propagation();
            callback(this, event, window, cx)
        }))
        .into_any_element()
}

pub(super) use super::zapper_avatar::{zapper_avatar, zapper_mascot};

pub(super) const ZAPPER_PANEL_WIDTH: f32 = 520.0;

/// A document worth listing: seeded placeholders stay empty, and the
/// instructions file only appears once rewritten. Until then it holds just
/// the purpose the Overview already shows.
#[cfg(feature = "appshots-fixture")]
fn is_document(file: &ZapperFile) -> bool {
    (file.path != "INSTRUCTIONS.md" || file.version > 1) && !file.content.trim().is_empty()
}

fn section_icon(tab: ZapperTab) -> &'static str {
    match tab {
        ZapperTab::Overview => icons::WIDGET,
        ZapperTab::Issues => icons::CHECKLIST,
        ZapperTab::Threads => icons::CHAT_ROUND_LINE,
        ZapperTab::Drive => icons::FOLDER_OPEN,
        ZapperTab::History => icons::HISTORY,
    }
}

/// Side of a collapsed section button.
const SECTION_BUTTON: f32 = 28.0;

/// Gap between section buttons.
const SECTION_GAP: f32 = 2.0;

/// Width of the expanded section button, sized for the longest visible
/// label so the group keeps one width whichever section is open.
fn section_width(visible: &[ZapperTab]) -> f32 {
    let label = visible.iter().map(|t| t.label_width()).fold(0.0, f32::max);
    // Padding, the 13px icon and the gap before the label.
    10.0 + 13.0 + 6.0 + label + 10.0
}

/// The newest version of `path`; history listings carry several.
fn latest_file<'a>(files: &'a [ZapperFile], path: &str) -> Option<&'a ZapperFile> {
    files
        .iter()
        .filter(|f| f.path == path)
        .max_by_key(|f| f.version)
}

/// The tab label for `url`: `#123`.
fn pull_request_label(url: &str) -> String {
    format!("#{}", url.rsplit('/').next().unwrap_or(""))
}

/// Where one Zapper conversation stands in the sidebar.
#[derive(Clone, Copy)]
enum ThreadPhase {
    Working,
    NeedsYou(zuse_protocol::ChatIndicator),
    /// No message has run yet, so it has no worktree or transcript.
    NotStarted,
    /// Waiting for a free slot: the Zapper already runs its maximum number
    /// of threads. Its chat is created when it starts.
    Queued,
    Done,
}

/// A Zapper thread row's phase. A thread waiting in the start queue has no
/// chat yet, so the queue decides, not the (absent) conversation.
fn worker_phase(worker: &ZapperWorker, chat: ThreadPhase) -> ThreadPhase {
    if worker.queued() {
        ThreadPhase::Queued
    } else {
        chat
    }
}

/// What the main pane shows for a selected Zapper thread that has no
/// conversation to render yet, instead of an empty transcript.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum ThreadPlaceholder {
    /// Waiting for a running thread to finish; `position` 1 starts next.
    Queued { objective: String, position: u32 },
    /// Delegated and starting: the engine hasn't reported its chat yet.
    Starting { objective: String },
}

pub(super) fn thread_placeholder(
    zappers: &[Zapper],
    chat_exists: impl Fn(&str) -> bool,
    selected: &str,
) -> Option<ThreadPlaceholder> {
    let worker = zappers
        .iter()
        .flat_map(|z| &z.workers)
        .find(|w| w.chat_id == selected)?;
    match worker.queue_position {
        Some(position) => Some(ThreadPlaceholder::Queued {
            objective: worker.objective.clone(),
            position,
        }),
        None if !chat_exists(selected) => Some(ThreadPlaceholder::Starting {
            objective: worker.objective.clone(),
        }),
        None => None,
    }
}

fn thread_phase(status: zuse_protocol::ChatIndicator, started: bool) -> ThreadPhase {
    use zuse_protocol::ChatIndicator as C;
    match status {
        C::Working => ThreadPhase::Working,
        C::AwaitingInput | C::Errored => ThreadPhase::NeedsYou(status),
        C::Completed | C::Idle if started => ThreadPhase::Done,
        C::Completed | C::Idle => ThreadPhase::NotStarted,
    }
}

/// Side of the status slot on Zapper and thread rows.
const MARK: f32 = 12.0;

/// A row's status, in the same vocabulary and size as project rows: the
/// working loader, a needs-you glyph, a check once done, a faint ring before
/// the first run.
fn thread_mark(
    key: String,
    phase: ThreadPhase,
    awaiting: Option<zuse_protocol::AwaitingInputKind>,
    theme: &Theme,
) -> AnyElement {
    let slot = || {
        div()
            .size(px(MARK))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
    };
    let glyph = |path: &'static str, size: f32, tone: gpui::Hsla| {
        slot()
            .child(icon(path).size(px(size)).text_color(tone))
            .into_any_element()
    };
    match phase {
        ThreadPhase::Working => slot()
            .child(crate::loaders::retro_loader(
                key,
                10.0,
                theme.loader_glyph(),
            ))
            .into_any_element(),
        ThreadPhase::NeedsYou(status) => {
            match super::spaces::status_glyph(status, false, false, awaiting, theme) {
                Some((path, tone)) => glyph(path, 12.0, tone),
                None => slot().into_any_element(),
            }
        }
        ThreadPhase::NotStarted => slot()
            .child(
                div()
                    .size(px(7.0))
                    .rounded_full()
                    .border_1()
                    .border_color(theme.text_faint.opacity(0.7)),
            )
            .into_any_element(),
        ThreadPhase::Queued => glyph(icons::STATUS_QUEUED, 12.0, theme.text_muted),
        ThreadPhase::Done => glyph(icons::CHECK, 11.0, theme.text_faint),
    }
}

impl Shell {
    /// The placeholder for the selected Zapper thread, when it has no
    /// conversation yet (queued, or delegated and still starting).
    pub(super) fn selected_thread_placeholder(&self, cx: &App) -> Option<ThreadPlaceholder> {
        let state = self.state.read(cx);
        let selected = state.selected_chat.as_deref()?;
        thread_placeholder(
            &state.zappers,
            |id| state.chats.iter().any(|c| c.id == id),
            selected,
        )
    }

    pub(super) fn render_thread_placeholder(
        &self,
        placeholder: ThreadPlaceholder,
        cx: &App,
    ) -> AnyElement {
        let theme = Theme::of(cx);
        let (glyph, heading, line, objective) = match placeholder {
            ThreadPlaceholder::Queued {
                objective,
                position,
            } => (
                icons::STATUS_QUEUED,
                "Queued",
                if position == 1 {
                    "Starts when a thread finishes. It's next in line.".to_owned()
                } else {
                    format!("Starts when a thread finishes. It's number {position} in line.")
                },
                objective,
            ),
            ThreadPlaceholder::Starting { objective } => (
                icons::CLOCK_CIRCLE,
                "Starting",
                "This thread's chat appears once it starts.".to_owned(),
                objective,
            ),
        };
        const OBJECTIVE: usize = 800;
        let objective = if objective.chars().count() > OBJECTIVE {
            format!("{}…", objective.chars().take(OBJECTIVE).collect::<String>())
        } else {
            objective
        };
        div()
            .debug_selector(|| "zapper-thread-placeholder".into())
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .items_center()
            .justify_center()
            .p(px(24.0))
            .child(
                div()
                    .max_w(px(460.0))
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .text_size(crate::typography::ui_rems(14.0))
                            .text_color(theme.text_strong)
                            .child(icon(glyph).size(px(14.0)).text_color(theme.text_muted))
                            .child(heading),
                    )
                    .child(
                        div()
                            .text_size(crate::typography::ui_rems(13.0))
                            .text_color(theme.text_muted)
                            .child(line),
                    )
                    .child(
                        div()
                            .mt(px(4.0))
                            .p(px(12.0))
                            .rounded(px(8.0))
                            .border_1()
                            .border_color(theme.border)
                            .text_size(crate::typography::ui_rems(12.5))
                            .text_color(theme.text)
                            .child(objective),
                    ),
            )
            .into_any_element()
    }

    pub(super) fn selected_zapper(&self, cx: &App) -> Option<Zapper> {
        let state = self.state.read(cx);
        state
            .zappers
            .iter()
            .find(|z| {
                let selected = state.selected_chat.as_ref();
                Some(&z.coordinator_chat_id) == selected
                    || z.previous_coordinator_chats
                        .iter()
                        .any(|c| Some(c) == selected)
            })
            .cloned()
    }
    pub(super) fn render_zapper_sidebar(
        &mut self,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let state = self.state.read(cx);
        if !state
            .engine()
            .is_some_and(|e| e.engine_info().supports(zuse_protocol::zappers::CAPABILITY))
        {
            return Empty.into_any_element();
        }
        let zappers = state.zappers.clone();
        // A Zapper page owns the main area, so no conversation reads as current.
        let creating = self.creating_zapper();
        let selected = state.selected_chat.clone().filter(|_| !creating);
        let titles: std::collections::HashMap<String, String> = state
            .chats
            .iter()
            .filter_map(|c| Some((c.id.clone(), c.title.clone()?)))
            .collect();
        let header = spaces::sidebar_section_heading(theme, "Zappers", "sidebar-zappers-label")
            .child(
                spaces::sidebar_header_button("new-zapper", icons::PLUS, theme)
                    .debug_selector(|| "new-zapper".into())
                    .aria_label("New Zapper")
                    .when(creating, |d| d.bg(theme.bg_selected))
                    .tip("New Zapper")
                    .on_click(cx.listener(|this, _, _, cx| this.open_new_zapper(cx))),
            );
        let mut list = div().flex().flex_col().px(px(8.0)).pb(px(8.0)).gap(px(1.0));
        for zapper in zappers {
            let id = zapper.coordinator_chat_id.clone();
            let active = selected.as_deref() == Some(id.as_str())
                || zapper
                    .previous_coordinator_chats
                    .iter()
                    .any(|c| Some(c) == selected.as_ref());
            // Open decisions plus issues the Zapper handed to the user.
            let needs = zapper
                .decisions
                .iter()
                .filter(|d| d.answer.is_none())
                .count()
                + zapper
                    .issues
                    .iter()
                    .filter(|i| i.details.status == ZapperIssueStatus::NeedsYou)
                    .count();
            let (coordinator, _) = self.zapper_thread_phase(&id, cx);
            // Finished threads leave the sidebar and stay under the Threads tab.
            let threads: Vec<_> = zapper
                .workers
                .iter()
                .filter(|worker| !self.zapper_thread_archived(&worker.chat_id, cx))
                .map(|worker| {
                    let (phase, awaiting) = self.zapper_thread_phase(&worker.chat_id, cx);
                    (worker, worker_phase(worker, phase), awaiting)
                })
                .filter(|(worker, phase, _)| {
                    !matches!(phase, ThreadPhase::Done)
                        || selected.as_deref() == Some(worker.chat_id.as_str())
                })
                .collect();
            // Threads show their own loader underneath, so the Zapper row
            // only spins for the coordinator itself.
            let working = matches!(coordinator, ThreadPhase::Working);
            let waiting = if needs == 1 {
                "1 thing waiting for you".to_owned()
            } else {
                format!("{needs} things waiting for you")
            };
            // Stays lit while its right-click menu is open.
            let menu_open = self
                .zapper_ui
                .menu
                .get()
                .is_some_and(|(menu_zapper, _)| menu_zapper == &zapper.id);
            list = list.child(
                div()
                    .id(SharedString::from(format!("zapper-{}", zapper.id)))
                    .role(gpui::Role::Button)
                    .aria_label(SharedString::from(zapper.config.name.clone()))
                    .h(px(26.0))
                    .px(px(6.0))
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .rounded(px(7.0))
                    .cursor_pointer()
                    .text_size(crate::typography::ui_rems(13.0))
                    .text_color(if active {
                        theme.text_strong
                    } else {
                        theme.text
                    })
                    .when(active, |d| d.bg(theme.bg_selected))
                    .when(!active && menu_open, |d| d.bg(theme.element_hover))
                    .when(!active, |d| d.hover(|d| d.bg(theme.element_hover)))
                    .child(zapper_mascot(&self.zapper_face(&zapper, cx), 16.0))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .child(zapper.config.name.clone()),
                    )
                    .when(zapper.status != ZapperStatus::Active, |d| {
                        d.child(
                            div()
                                .text_size(crate::typography::ui_rems(11.0))
                                .text_color(theme.text_faint)
                                .child(format!("{:?}", zapper.status)),
                        )
                    })
                    .when(working, |d| {
                        d.child(thread_mark(
                            format!("zapper-working-{}", zapper.id),
                            ThreadPhase::Working,
                            None,
                            theme,
                        ))
                    })
                    .when(!working && needs > 0, |d| {
                        d.child(
                            div()
                                .id(SharedString::from(format!(
                                    "zapper-decisions-{}",
                                    zapper.id
                                )))
                                .aria_label(SharedString::from(waiting.clone()))
                                .tip(waiting.clone())
                                .size(px(MARK))
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(div().size(px(6.0)).rounded_full().bg(theme.warning)),
                        )
                    })
                    .on_click(cx.listener(move |this, _, _, cx| this.open_chat(id.clone(), cx)))
                    .on_mouse_down(MouseButton::Right, {
                        let zapper_id = zapper.id.clone();
                        cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                            if this.zapper_ui.menu.is_open() {
                                this.slack_links.menu_closed();
                            }
                            this.zapper_ui.slack_open = false;
                            this.zapper_ui.submenu.reset();
                            this.zapper_ui
                                .menu
                                .open((zapper_id.clone(), event.position));
                            cx.stop_propagation();
                            cx.notify();
                        })
                    }),
            );
            // A Zapper's threads live under it instead of under their projects.
            for (worker, phase, awaiting) in threads {
                let chat = worker.chat_id.clone();
                let current = selected.as_deref() == Some(chat.as_str());
                let title = titles
                    .get(&chat)
                    .cloned()
                    .unwrap_or_else(|| worker.objective.clone());
                let row_group = SharedString::from(format!("zapper-sidebar-thread-{chat}"));
                let archive = {
                    let zapper = zapper.clone();
                    let chat = chat.clone();
                    thread_action(
                        format!("zapper-sidebar-archive-{chat}"),
                        icons::ARCHIVE_MINIMALISTIC,
                        "Archive",
                        theme,
                        move |this, _, _, cx| this.archive_zapper_thread(&zapper, chat.clone(), cx),
                        cx,
                    )
                };
                list = list.child(
                    div()
                        .id(SharedString::from(format!("zapper-thread-{chat}")))
                        .group(row_group.clone())
                        .role(gpui::Role::Button)
                        .aria_label(SharedString::from(title.clone()))
                        .h(px(26.0))
                        // Indented so the status sits under the Zapper's name.
                        .pl(px(30.0))
                        .pr(px(6.0))
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .relative()
                        .rounded(px(7.0))
                        .cursor_pointer()
                        .text_size(crate::typography::ui_rems(12.5))
                        .text_color(if current {
                            theme.text_strong
                        } else {
                            theme.text_muted
                        })
                        .when(current, |d| d.bg(theme.bg_selected))
                        .when(!current && self.chat_menu_targets(&chat), |d| {
                            d.bg(theme.element_hover).text_color(theme.text)
                        })
                        .when(!current, |d| {
                            d.hover(|d| d.bg(theme.element_hover).text_color(theme.text))
                        })
                        .child(thread_mark(
                            format!("zapper-thread-working-{chat}"),
                            phase,
                            awaiting,
                            theme,
                        ))
                        .child(div().flex_1().min_w_0().truncate().child(title))
                        .when(matches!(phase, ThreadPhase::NotStarted), |d| {
                            d.tip("Hasn't started yet")
                        })
                        .when(matches!(phase, ThreadPhase::Queued), |d| {
                            d.child(
                                div()
                                    .flex_none()
                                    .text_size(crate::typography::ui_rems(11.0))
                                    .text_color(theme.text_faint)
                                    .child("Queued"),
                            )
                            .tip("Queued · starts when a thread finishes")
                        })
                        .child(
                            div()
                                .absolute()
                                .right(px(1.0))
                                .top(px(0.0))
                                .bottom(px(0.0))
                                .flex()
                                .items_center()
                                .invisible()
                                .group_hover(row_group, |d| d.visible())
                                .child(archive),
                        )
                        .on_mouse_down(MouseButton::Right, {
                            let chat = chat.clone();
                            cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                                this.chat_menu.open(super::ChatMenuState {
                                    tab: None,
                                    chat_id: chat.clone(),
                                    position: event.position,
                                    page: super::ChatMenuPage::Root,
                                    sidebar_actions: false,
                                });
                                cx.stop_propagation();
                                cx.notify();
                            })
                        })
                        .on_click(
                            cx.listener(move |this, _, _, cx| this.open_chat(chat.clone(), cx)),
                        ),
                );
            }
        }
        div()
            .flex()
            .flex_col()
            .child(header)
            .child(list)
            .into_any_element()
    }
    /// Archived threads leave every Zapper list and live under Archived.
    fn zapper_thread_archived(&self, chat: &str, cx: &App) -> bool {
        self.state.read(cx).chat_is_archived(chat)
            || self
                .zapper_ui
                .archiving
                .get(chat)
                .is_some_and(|at| at.elapsed() < std::time::Duration::from_secs(10))
    }

    /// The Zapper a thread works for, if any.
    pub(super) fn zapper_of_thread(&self, chat_id: &str, cx: &App) -> Option<Zapper> {
        self.state
            .read(cx)
            .zappers
            .iter()
            .find(|z| z.workers.iter().any(|w| w.chat_id == chat_id))
            .cloned()
    }

    pub(super) fn close_zapper_menu(&mut self, cx: &mut Context<Self>) {
        self.zapper_ui.submenu.reset();
        if self.zapper_ui.menu.is_open() {
            self.slack_links.menu_closed();
        }
        if self.zapper_ui.menu.begin_close() {
            popover::reap_popup(cx, |shell: &mut Self| &mut shell.zapper_ui.menu);
            cx.notify();
        }
    }

    /// Escape closes the panel's `…` menu, the Zapper menu, then its delete
    /// confirmation.
    pub(super) fn dismiss_zapper_overlay(&mut self, cx: &mut Context<Self>) -> bool {
        if self.zapper_ui.overflow.is_open() {
            self.close_zapper_overflow(cx);
            return true;
        }
        if self.zapper_ui.menu.is_open() {
            self.close_zapper_menu(cx);
            return true;
        }
        if self.zapper_ui.delete_confirm.take().is_some() {
            cx.notify();
            return true;
        }
        false
    }

    pub(super) fn zapper_overlay_open(&self) -> bool {
        self.zapper_ui.menu.get().is_some()
            || self.zapper_ui.overflow.get().is_some()
            || self.zapper_ui.delete_confirm.is_some()
    }

    /// The settings page of `zapper` closes when the Zapper itself goes away.
    fn after_removing(&self, zapper: &Zapper) -> ZapperAfter {
        if self
            .zapper_ui
            .page
            .as_ref()
            .is_some_and(|page| page.id == zapper.id)
        {
            ZapperAfter::LeavePage
        } else {
            ZapperAfter::Stay
        }
    }

    pub(super) fn render_zapper_overlays(
        &mut self,
        viewport: gpui::Size<Pixels>,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let theme = Theme::of(cx).for_popup();
        let zappers = self.state.read(cx).zappers.clone();
        let mut overlays = Vec::new();
        if let Some((zapper_id, position)) = self.zapper_ui.menu.get().cloned() {
            match zappers.iter().find(|z| z.id == zapper_id).cloned() {
                Some(zapper) => {
                    let row = |key: &str, glyph: &'static str, label: &str, danger: bool| {
                        let tone = if danger {
                            theme.danger
                        } else {
                            theme.text_muted
                        };
                        popover::menu_row(&theme, false, format!("zapper-menu-{key}-{zapper_id}"))
                            .id(SharedString::from(format!("zapper-menu-{key}")))
                            .when(danger, |d| d.text_color(theme.danger))
                            .child(icon(glyph).size(px(16.0)).text_color(tone))
                            .child(SharedString::from(label.to_owned()))
                    };
                    let z = zapper.clone();
                    let new_thread = row("thread", icons::PLUS, "New thread", false).on_click(
                        cx.listener(move |this, _, _, cx| {
                            this.close_zapper_menu(cx);
                            this.open_zapper_dialog(z.clone(), Kind::Thread(None), cx);
                        }),
                    );
                    let z = zapper.clone();
                    let settings = row("settings", icons::SETTINGS, "Settings", false).on_click(
                        cx.listener(move |this, _, _, cx| {
                            this.close_zapper_menu(cx);
                            this.open_zapper_editor(z.clone(), cx);
                        }),
                    );
                    let z = zapper.clone();
                    let archive = row(
                        "archive",
                        icons::ARCHIVE_MINIMALISTIC,
                        "Archive with threads",
                        false,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.close_zapper_menu(cx);
                        let after = this.after_removing(&z);
                        this.send_zapper_command(
                            z.id.clone(),
                            Some(z.revision),
                            ZapperAction::SetStatus {
                                status: ZapperStatus::Archived,
                            },
                            after,
                            cx,
                        );
                    }));
                    let z = zapper.clone();
                    let delete = row("delete", icons::TRASH_BIN_MINIMALISTIC, "Delete…", true)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.close_zapper_menu(cx);
                            this.zapper_ui.delete_confirm = Some(z.id.clone());
                            cx.notify();
                        }));
                    use super::slack_link::{SubmenuHost, ZAPPER_MENU_ROW_SLACK};
                    let slack = self.render_slack_trigger(
                        SubmenuHost::Zapper,
                        &zapper.coordinator_chat_id,
                        self.zapper_ui.slack_open,
                        &theme,
                        cx,
                    );
                    let host = SubmenuHost::Zapper;
                    let new_thread = self.submenu_hover_row(host, 0, new_thread, cx);
                    let settings = self.submenu_hover_row(host, 1, settings, cx);
                    let archive = self.submenu_hover_row(host, 2, archive, cx);
                    let slack = self.submenu_hover_row(host, ZAPPER_MENU_ROW_SLACK, slack, cx);
                    let delete = self.submenu_hover_row(host, 4, delete, cx);
                    let menu = popover::popover_card(&theme)
                        .w(px(216.0))
                        .flex()
                        .flex_col()
                        .on_mouse_down_out(cx.listener(|this, event: &MouseDownEvent, _, cx| {
                            // The Slack submenu floats outside this card.
                            if this
                                .zapper_ui
                                .submenu
                                .bounds
                                .is_some_and(|bounds| bounds.contains(&event.position))
                            {
                                return;
                            }
                            this.close_zapper_menu(cx)
                        }))
                        .child(new_thread)
                        .child(settings)
                        .when(zapper.status != ZapperStatus::Archived, |menu| {
                            menu.child(archive)
                        })
                        .child(slack)
                        .child(popover::menu_separator())
                        .child(delete)
                        .into_any_element();
                    overlays.push(popover::menu_at(
                        "zapper-context-menu",
                        position,
                        menu,
                        self.zapper_ui.menu.closing_since(),
                    ));
                }
                // The Zapper vanished under the open menu.
                None => self.close_zapper_menu(cx),
            }
        }
        if let Some(zapper_id) = self.zapper_ui.delete_confirm.clone() {
            let Some(zapper) = zappers.into_iter().find(|z| z.id == zapper_id) else {
                self.zapper_ui.delete_confirm = None;
                return overlays;
            };
            let name = crate::transcript::single_line(&zapper.config.name);
            let card = popover::dialog_card(&theme)
                .child(popover::dialog_title(&theme, "Delete Zapper?"))
                .child(div().mt(px(6.0)).child(popover::dialog_body(
                    &theme,
                    format!(
                        "\u{201C}{name}\u{201D} stops and is removed. Its threads, documents and repository files are kept."
                    ),
                )))
                .child(
                    popover::dialog_footer(&theme)
                        .child(
                            popover::btn_ghost(
                                &theme,
                                &tr(cx, msg::common::cancel),
                                "delete-zapper-cancel",
                            )
                            .id("delete-zapper-cancel")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.zapper_ui.delete_confirm = None;
                                cx.notify();
                            })),
                        )
                        .child(
                            popover::btn_danger(&theme, "Delete")
                                .id("delete-zapper-confirm")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.zapper_ui.delete_confirm = None;
                                    let after = this.after_removing(&zapper);
                                    this.send_zapper_command(
                                        zapper.id.clone(),
                                        Some(zapper.revision),
                                        ZapperAction::Delete,
                                        after,
                                        cx,
                                    );
                                })),
                        ),
                )
                .into_any_element();
            overlays.push(popover::modal("delete-zapper-dialog", viewport, card));
        }
        overlays
    }

    pub(super) fn archive_zapper_thread(
        &mut self,
        zapper: &Zapper,
        chat_id: String,
        cx: &mut Context<Self>,
    ) {
        if self.zapper_ui.pending {
            return;
        }
        // By id: a queued thread has no chat row yet.
        let viewing = self.state.read(cx).selected_chat.as_deref() == Some(chat_id.as_str());
        // Don't leave the user on a thread that just left the list.
        if viewing {
            self.open_chat(zapper.coordinator_chat_id.clone(), cx);
        }
        self.zapper_ui
            .archiving
            .insert(chat_id.clone(), std::time::Instant::now());
        self.send_zapper_command(
            zapper.id.clone(),
            Some(zapper.revision),
            ZapperAction::ArchiveWorker { chat_id },
            ZapperAfter::Stay,
            cx,
        );
        cx.notify();
    }

    fn zapper_thread_phase(
        &self,
        chat: &str,
        cx: &App,
    ) -> (ThreadPhase, Option<zuse_protocol::AwaitingInputKind>) {
        let state = self.state.read(cx);
        let Some(row) = state.chats.iter().find(|c| c.id == chat) else {
            return (ThreadPhase::NotStarted, None);
        };
        let status = state.display_status_for(row, chrono::Utc::now());
        let awaiting = state.session_for(chat).and_then(|s| s.awaiting_input);
        (
            thread_phase(status, row.last_message_at.is_some()),
            awaiting,
        )
    }
    /// Live run state of one thread, ignoring rows a crashed host left behind.
    fn zapper_thread_status(&self, chat: &str, cx: &App) -> Option<zuse_protocol::SessionStatus> {
        self.state
            .read(cx)
            .sessions
            .iter()
            .find(|s| s.chat_id == chat && (chrono::Utc::now() - s.updated_at).num_seconds() < 45)
            .map(|s| s.status.clone())
    }
    fn zapper_status_color(&self, chat: &str, theme: &Theme, cx: &App) -> gpui::Hsla {
        use zuse_protocol::SessionStatus as S;
        match self.zapper_thread_status(chat, cx) {
            Some(S::Working) => theme.busy,
            Some(S::AwaitingInput) => theme.warning,
            Some(S::Errored) => theme.danger,
            _ => theme.text_faint.opacity(0.5),
        }
    }
    fn zapper_history_view(&self) -> bool {
        *self.zapper_ui.tabs.active() == ZapperPage::History
    }

    /// Show `page`, opening it as a tab when it is an issue, document, pull
    /// request or the history. Leaves the Zapper's settings if they are open.
    pub(super) fn show_zapper_page(&mut self, page: ZapperPage, cx: &mut Context<Self>) {
        if self.zapper_editor_open(cx) {
            self.zapper_ui.page = None;
        }
        let history = self.zapper_history_view();
        // Reopening a pull request reloads details that may be out of date.
        if let (ZapperPage::PullRequest(url), Some((loaded, existing))) =
            (&page, &self.zapper_ui.pull_request_page)
            && url == loaded
        {
            existing.update(cx, |existing, cx| existing.refresh_if_stale(cx));
        }
        self.zapper_ui.tabs.show(page);
        self.zapper_page_changed(history);
        self.reveal_active_zapper_tab();
        cx.notify();
    }

    /// Start a different Zapper's panel on a bare Overview, releasing what
    /// the previous one's tabs held.
    fn sync_zapper_tabs(&mut self, zapper_id: &str, cx: &mut Context<Self>) {
        if self.zapper_ui.tabs_zapper.as_deref() == Some(zapper_id) {
            return;
        }
        let history = self.zapper_history_view();
        for page in self.zapper_ui.tabs.open_tabs().to_vec() {
            self.release_zapper_page(&page, cx);
        }
        self.zapper_ui.tabs.reset();
        self.zapper_page_changed(history);
        self.zapper_ui.tabs_zapper = Some(zapper_id.to_owned());
    }

    /// A coordinator chat's opener (a pull request link, a subagent chip, a
    /// diff): show `page` as a tab in the Zapper panel, bringing the panel
    /// back if it was hidden.
    pub(super) fn show_in_zapper_panel(&mut self, page: ZapperPage, cx: &mut Context<Self>) {
        let Some(zapper) = self.selected_zapper(cx) else {
            return;
        };
        self.sync_zapper_tabs(&zapper.id, cx);
        self.zapper_ui.panel_hidden = false;
        self.show_zapper_page(page, cx);
    }

    /// The checkout review open as a Zapper panel tab, if any.
    pub(super) fn zapper_review_page(&self, cx: &App) -> Option<u64> {
        self.zapper_ui
            .tabs
            .open_tabs()
            .iter()
            .find_map(|page| match page {
                ZapperPage::Diff(id) => self
                    .diffs
                    .get(id)
                    .filter(|changes| changes.read(cx).is_review())
                    .map(|_| *id),
                _ => None,
            })
    }

    #[cfg(test)]
    pub(super) fn zapper_tabs(&self) -> &ZapperPanelTabs {
        &self.zapper_ui.tabs
    }

    /// Close a tab and return to the page shown before it.
    pub(super) fn close_zapper_page(&mut self, page: &ZapperPage, cx: &mut Context<Self>) {
        let history = self.zapper_history_view();
        if self.zapper_ui.tabs.close(page) {
            self.release_zapper_page(page, cx);
            self.zapper_page_changed(history);
            self.reveal_active_zapper_tab();
            cx.notify();
        }
    }

    /// Bring the active tab into view; with a section active, return the
    /// open tabs to their start so a shrunk strip isn't left scrolled.
    fn reveal_active_zapper_tab(&self) {
        let tabs = &self.zapper_ui.tabs;
        let ix = tabs.open_tabs().iter().position(|p| tabs.is_active(p));
        if !tabs.open_tabs().is_empty() {
            self.zapper_ui.open_scroll.scroll_to_item(ix.unwrap_or(0));
        }
    }

    /// History lists every version, the other pages only the latest: moving
    /// in or out of it reloads the files.
    fn zapper_page_changed(&mut self, history: bool) {
        if history != self.zapper_history_view() {
            self.zapper_ui.loaded = None;
        }
    }

    fn close_zapper_overflow(&mut self, cx: &mut Context<Self>) {
        if self.zapper_ui.overflow.begin_close() {
            popover::reap_popup(cx, |shell: &mut Self| &mut shell.zapper_ui.overflow);
            cx.notify();
        }
    }

    /// Fixed section buttons, the open tabs, then the page actions and the
    /// `…` menu. Sections are icons; the current one shows its label at a
    /// fixed width so nothing after it moves when switching.
    fn zapper_tab_bar(
        &self,
        zapper: &Zapper,
        visible: &[ZapperTab],
        editing: bool,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let tabs = &self.zapper_ui.tabs;
        let expanded_section = tabs.expanded_section().filter(|_| !editing);
        let expanded_width = crate::typography::ui_rems(section_width(visible));
        let mut sections = div()
            .flex()
            .flex_none()
            .items_center()
            // Sized to its buttons: with an issue or pull request tab
            // active every section is an icon, and the tabs follow the
            // separator right after them instead of after reserved room.
            .gap(crate::typography::ui_rems(SECTION_GAP));
        for &tab in visible {
            let label = tab.label();
            let expanded = Some(tab) == expanded_section;
            let active = expanded;
            let color = if active {
                super::tabs::active_tab_text_color(theme)
            } else {
                theme.text_muted
            };
            let button = crate::surface_chrome::workspace_tab(
                SharedString::from(format!("zapper-tab-{label}")),
                active,
                theme,
            )
            .role(gpui::Role::Tab)
            .aria_label(label)
            .aria_selected(active)
            .debug_selector(move || format!("zapper-tab-{label}"))
            .h(px(SECTION_BUTTON))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .gap(px(6.0))
            .cursor_pointer()
            .text_size(crate::typography::ui_rems(12.5))
            .text_color(color)
            .when(active, |d| d.font_weight(gpui::FontWeight::MEDIUM))
            .child(
                icon(section_icon(tab))
                    .size(px(13.0))
                    .flex_none()
                    .text_color(color),
            )
            .map(|d| {
                if expanded {
                    d.w(expanded_width)
                        .child(div().whitespace_nowrap().child(label))
                } else {
                    d.w(crate::typography::ui_rems(SECTION_BUTTON)).tip(label)
                }
            })
            .on_click(cx.listener(move |this, _, _, cx| {
                this.show_zapper_page(ZapperPage::Section(tab), cx);
            }));
            sections = sections.child(button);
        }

        let mut open = div()
            .id("zapper-open-tabs")
            .flex_1()
            .min_w_0()
            .flex()
            .items_center()
            .gap(px(2.0))
            .overflow_x_scroll()
            .track_scroll(&self.zapper_ui.open_scroll);
        for page in tabs.open_tabs() {
            open = open.child(self.zapper_page_tab(
                zapper,
                page,
                !editing && tabs.is_active(page),
                theme,
                cx,
            ));
        }

        let icon_button = |id: &'static str, path: &'static str, tip: &'static str| {
            div()
                .id(id)
                .role(gpui::Role::Button)
                .aria_label(tip)
                .size(px(SECTION_BUTTON))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(7.0))
                .cursor_pointer()
                .hover(|d| d.bg(theme.element_hover))
                .tip(tip)
                .child(icon(path).size(px(13.0)).text_color(theme.text_muted))
        };
        let overflow_open = self.zapper_ui.overflow.get().is_some();
        let overflow = icon_button("zapper-more", icons::MORE_HORIZONTAL, "More")
            .debug_selector(|| "zapper-more".into())
            .aria_expanded(overflow_open)
            .when(overflow_open, |d| d.bg(theme.element_hover))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, _| {
                    window.prevent_default();
                    this.zapper_ui.overflow.note_trigger_press();
                }),
            )
            .on_click(cx.listener(|this, _, _, cx| {
                cx.stop_propagation();
                if this.zapper_ui.overflow.take_press_was_open() {
                    this.close_zapper_overflow(cx);
                } else {
                    this.zapper_ui.overflow.open(());
                    cx.notify();
                }
            }));
        let overflow = if overflow_open {
            let closing = self.zapper_ui.overflow.closing_since();
            let menu = self.zapper_overflow_menu(theme, cx);
            overflow
                .relative()
                .child(popover::anchored_menu_below_end(
                    "zapper-more-menu",
                    menu,
                    closing,
                ))
                .into_any_element()
        } else {
            overflow.into_any_element()
        };
        let new_doc = zapper.clone();
        let edit = zapper.clone();
        div()
            .flex()
            .items_center()
            .gap(px(4.0))
            .h(px(40.0))
            .flex_none()
            .px(px(8.0))
            .border_b_1()
            .border_color(theme.border)
            .child(sections)
            .child(
                div()
                    .w(px(1.0))
                    .h(px(16.0))
                    .flex_none()
                    .mx(px(2.0))
                    .when(!tabs.open_tabs().is_empty(), |d| d.bg(theme.border)),
            )
            .child(open)
            .when(
                *tabs.active() == ZapperPage::Section(ZapperTab::Drive) && !editing,
                |d| {
                    let new_folder = new_doc.clone();
                    let (file_key, folder_key) = (new_doc.clone(), new_folder.clone());
                    let new_file =
                        move |this: &mut Shell, zapper: &Zapper, cx: &mut Context<Shell>| {
                            let file = ZapperFile {
                                path: this.zapper_tree_prefix(),
                                ..Default::default()
                            };
                            this.open_zapper_dialog(zapper.clone(), Kind::File(file), cx)
                        };
                    let add_folder =
                        move |this: &mut Shell, zapper: &Zapper, cx: &mut Context<Shell>| {
                            let prefix = this.zapper_tree_prefix();
                            this.open_zapper_dialog(zapper.clone(), Kind::Folder(prefix), cx)
                        };
                    let activates = |event: &gpui::KeyDownEvent| {
                        matches!(event.keystroke.key.as_str(), "enter" | "space")
                    };
                    d.child(
                        icon_button("zapper-new-document", icons::DOCUMENT_ADD, "New file")
                            .tab_index(0)
                            .on_click(
                                cx.listener(move |this, _, _, cx| new_file(this, &new_doc, cx)),
                            )
                            .on_key_down(cx.listener(
                                move |this, event: &gpui::KeyDownEvent, _, cx| {
                                    if activates(event) {
                                        cx.stop_propagation();
                                        new_file(this, &file_key, cx)
                                    }
                                },
                            )),
                    )
                    .child(
                        icon_button("zapper-new-folder", icons::FOLDER_ADD, "New folder")
                            .tab_index(0)
                            .on_click(
                                cx.listener(move |this, _, _, cx| {
                                    add_folder(this, &new_folder, cx)
                                }),
                            )
                            .on_key_down(cx.listener(
                                move |this, event: &gpui::KeyDownEvent, _, cx| {
                                    if activates(event) {
                                        cx.stop_propagation();
                                        add_folder(this, &folder_key, cx)
                                    }
                                },
                            )),
                    )
                },
            )
            .child(overflow)
            .child(
                icon_button("zapper-edit", icons::PEN, "Edit Zapper")
                    .when(editing, |d| d.bg(theme.element_active))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if this.zapper_editor_open(cx) {
                            this.zapper_ui.page = None;
                            cx.notify();
                        } else {
                            this.open_zapper_editor(edit.clone(), cx);
                        }
                    })),
            )
            .into_any_element()
    }

    /// One closable tab: a glyph, a short title and the shared close button.
    fn zapper_page_tab(
        &self,
        zapper: &Zapper,
        page: &ZapperPage,
        active: bool,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let glyph = |path: &'static str| {
            icon(path)
                .size(px(13.0))
                .flex_none()
                .text_color(theme.text_muted)
                .into_any_element()
        };
        let (key, title, tip, leading) = match page {
            ZapperPage::Issue(id) => {
                let issue = zapper.issues.iter().find(|i| &i.id == id);
                (
                    format!("issue-{id}"),
                    issue.map_or_else(|| "New issue".to_owned(), ZapperIssue::code),
                    issue.map(|i| i.details.title.clone()),
                    zuse_viewport::zapper_issues::status_icon(
                        issue.map_or(ZapperIssueStatus::Todo, |i| i.details.status),
                    )
                    .flex_none()
                    .into_any_element(),
                )
            }
            ZapperPage::File(path) => (
                format!("file-{path}"),
                path.clone(),
                None,
                glyph(icons::DOCUMENT),
            ),
            ZapperPage::PullRequest(url) => (
                format!("pr-{url}"),
                pull_request_label(url),
                Some(url.clone()),
                glyph(icons::PULL_REQUEST),
            ),
            ZapperPage::History => (
                "history".into(),
                "History".into(),
                None,
                glyph(icons::HISTORY),
            ),
            ZapperPage::Subagent(id) => {
                let tab = self.subagent_tabs.get(id);
                (
                    format!("subagent-{id}"),
                    tab.map_or_else(|| "Subagent".to_owned(), |t| t.title.to_string()),
                    tab.map(|t| t.title.to_string()),
                    glyph(icons::BOT),
                )
            }
            ZapperPage::Diff(id) => (
                format!("diff-{id}"),
                "Diff".into(),
                None,
                glyph(icons::GIT_COMPARE),
            ),
            ZapperPage::Section(_) => return Empty.into_any_element(),
        };
        let group = SharedString::from(format!("zapper-page-{key}"));
        let selector = group.to_string();
        let (show, close, middle) = (page.clone(), page.clone(), page.clone());
        crate::surface_chrome::workspace_tab(group.clone(), active, theme)
            .group(group.clone())
            .role(gpui::Role::Tab)
            .aria_label(SharedString::from(title.clone()))
            .aria_selected(active)
            .debug_selector(move || selector.clone())
            .cursor_pointer()
            .flex_none()
            .max_w(px(150.0))
            .h(px(SECTION_BUTTON))
            .pl(px(8.0))
            .pr(px(4.0))
            .flex()
            .items_center()
            .gap(px(6.0))
            .text_size(crate::typography::ui_rems(12.5))
            .text_color(if active {
                super::tabs::active_tab_text_color(theme)
            } else {
                theme.text_muted
            })
            .when(active, |d| d.font_weight(gpui::FontWeight::MEDIUM))
            .when_some(tip, |d, tip| {
                d.tip(tip)
                    .tooltip_show_delay(std::time::Duration::from_millis(350))
            })
            .child(leading)
            .child(div().min_w_0().truncate().child(title))
            .child(
                super::tabs::tab_close_button(
                    SharedString::from(format!("zapper-page-close-{key}")),
                    "Close tab",
                    active,
                    group,
                    theme,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.close_zapper_page(&close, cx);
                })),
            )
            .on_mouse_down(
                MouseButton::Middle,
                cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.close_zapper_page(&middle, cx);
                }),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                this.show_zapper_page(show.clone(), cx);
            }))
            .into_any_element()
    }

    /// The `…` menu: pages that don't need a permanent tab.
    fn zapper_overflow_menu(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let theme = theme.for_popup();
        let history = popover::menu_row(&theme, false, "zapper-more-history")
            .id("zapper-more-history-row")
            .debug_selector(|| "zapper-more-history-row".into())
            .role(gpui::Role::MenuItem)
            .aria_label("History")
            .gap(px(10.0))
            .text_size(crate::typography::text_sm())
            .py(px(5.0))
            .child(
                icon(icons::HISTORY)
                    .size(px(14.0))
                    .flex_none()
                    .text_color(theme.text),
            )
            .child(div().flex_1().min_w_0().truncate().child("History"))
            .child(
                div()
                    .text_size(crate::typography::text_xs())
                    .text_color(theme.text_muted)
                    .child("Document versions"),
            )
            .on_click(cx.listener(|this, _, _, cx| {
                this.close_zapper_overflow(cx);
                this.show_zapper_page(ZapperPage::History, cx);
            }));
        popover::popover_card(&theme)
            .w(px(240.0))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| this.close_zapper_overflow(cx)))
            .child(div().flex().flex_col().child(history))
            .into_any_element()
    }

    pub(super) fn render_zapper_inspector(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(zapper) = self.selected_zapper(cx) else {
            return Empty.into_any_element();
        };
        let theme = Theme::of(cx).clone();
        self.sync_zapper_tabs(&zapper.id, cx);
        let key = (zapper.id.clone(), zapper.revision, zapper.activity_revision);
        if self.zapper_ui.loaded.as_ref() != Some(&key) && !self.zapper_ui.loading {
            if let Some(engine) = self.state.read(cx).engine().cloned() {
                self.zapper_ui.loading = true;
                let requested_history = self.zapper_history_view();
                let requested_zapper = zapper.id.clone();
                let request = serde_json::json!({"zapperId":zapper.id,"targetDeviceId":zapper.config.home_device_id,"history":requested_history});
                cx.spawn(async move |this, cx| {
                    let result = engine
                        .client()
                        .call(zuse_protocol::control::ZAPPER_FILES, request.clone())
                        .await;
                    let events = engine
                        .client()
                        .call(zuse_protocol::control::ZAPPER_EVENTS, request)
                        .await;
                    let _ = this.update(cx, |this, cx| {
                        this.zapper_ui.loading = false;
                        if let Ok(value) = events {
                            if let Ok(events) = serde_json::from_value(value) {
                                this.zapper_ui.events = events;
                            }
                        }
                        // A tab switch mid-flight wants the other file set;
                        // leaving `loaded` unset makes the next render fetch it.
                        if this.zapper_history_view() == requested_history
                            && let Ok(value) = result
                            && let Ok(files) = serde_json::from_value(value)
                        {
                            this.zapper_ui.files = files;
                            this.zapper_ui.files_zapper = Some(requested_zapper);
                            this.zapper_ui.loaded = Some(key);
                        }
                        cx.notify();
                    });
                })
                .detach();
            }
        }
        let files = if self.zapper_ui.files_zapper.as_deref() == Some(zapper.id.as_str()) {
            self.zapper_ui.files.clone()
        } else {
            vec![]
        };
        let visible = ZAPPER_SECTIONS;
        let editing = self.zapper_editor_open(cx);
        let tab_bar = self.zapper_tab_bar(&zapper, &visible, editing, &theme, cx);
        if editing {
            let editor = self.render_zapper_editor(&zapper, window, cx);
            return self.zapper_inspector_frame(&theme, tab_bar, editor);
        }
        // A subagent or a diff opened from the coordinator chat fills the
        // panel below its strip, as it would the surface host.
        let surface = match *self.zapper_ui.tabs.active() {
            ZapperPage::Subagent(id) => Some(RightSurface::Subagent(id)),
            ZapperPage::Diff(id) => Some(RightSurface::Diff(id)),
            _ => None,
        };
        if let Some(surface) = surface {
            let content = self
                .render_surface_body(surface, cx)
                .unwrap_or_else(|| Empty.into_any_element());
            return self.zapper_inspector_frame(&theme, tab_bar, content);
        }
        if let ZapperPage::PullRequest(url) = self.zapper_ui.tabs.active().clone() {
            let page = self
                .zapper_ui
                .pull_request_page
                .as_ref()
                .filter(|(loaded, _)| loaded == &url)
                .map(|(_, page)| page.clone());
            if let Some(page) = page {
                // Merge, Open on GitHub and the More menu, as in a chat's PR pane.
                let toolbar = page.update(cx, |page, cx| page.toolbar(cx));
                let mut content = div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .child(crate::surface_chrome::flowing_header().child(toolbar));
                if let Some(issue) = zapper
                    .issues
                    .iter()
                    .find(|i| i.pull_requests.iter().any(|p| p.url == url))
                {
                    let id = issue.id.clone();
                    let mut links = div()
                        .flex()
                        .items_center()
                        .gap(px(12.))
                        .px(px(16.))
                        .py(px(10.))
                        .child(action(
                            "pr-linked-issue",
                            format!("‹ {} · {}", issue.code(), issue.details.title),
                            &theme,
                            move |this, _, _, cx| {
                                this.show_zapper_page(ZapperPage::Issue(id.clone()), cx);
                            },
                            cx,
                        ));
                    if let Some(chat) = issue.threads.first().cloned() {
                        links = links.child(action(
                            "pr-linked-thread",
                            "Thread ↗",
                            &theme,
                            move |this, _, _, cx| this.open_chat(chat.clone(), cx),
                            cx,
                        ));
                    }
                    content = content.child(links);
                }
                return self.zapper_inspector_frame(
                    &theme,
                    tab_bar,
                    content.child(page).into_any_element(),
                );
            }
            let device = zapper.config.home_device_id.clone();
            // Page construction starts RPC reads. Defer it until this render
            // finishes, and keep it inside the Zapper inspector's own navigation.
            cx.defer_in(window, move |this, window, cx| {
                if *this.zapper_ui.tabs.active() != ZapperPage::PullRequest(url.clone())
                    || this
                        .zapper_ui
                        .pull_request_page
                        .as_ref()
                        .is_some_and(|(loaded, _)| loaded == &url)
                {
                    return;
                }
                let page = cx.new(|cx| {
                    crate::pull_request_detail::PullRequestDetailPage::new(
                        this.state.clone(),
                        url.clone(),
                        Some(device),
                        this.pull_request_cache.clone(),
                        None,
                        window,
                        cx,
                    )
                });
                this.zapper_ui.pull_request_subscription =
                    Some(cx.observe(&page, |_, _, cx| cx.notify()));
                this.zapper_ui.pull_request_page = Some((url, page));
                cx.notify();
            });
            return self.zapper_inspector_frame(
                &theme,
                tab_bar,
                div()
                    .p(px(16.))
                    .text_color(theme.text_muted)
                    .child("Loading pull request…")
                    .into_any_element(),
            );
        }
        let body = match self.zapper_ui.tabs.active().clone() {
            ZapperPage::Section(ZapperTab::Overview) => {
                let header = self.zapper_overview(&zapper, &theme, cx);
                let work = self.zapper_issue_surface(&zapper, true, cx);
                let repositories = self.zapper_repositories(&zapper, &theme, cx);
                div()
                    .flex()
                    .flex_col()
                    .gap(px(20.))
                    .child(header)
                    .child(work)
                    .child(repositories)
                    .into_any_element()
            }
            ZapperPage::Section(ZapperTab::Issues) | ZapperPage::Issue(_) => {
                self.zapper_issue_surface(&zapper, false, cx)
            }
            ZapperPage::Section(ZapperTab::Threads) => self.zapper_threads(&zapper, &theme, cx),
            ZapperPage::Section(ZapperTab::Drive) => self.zapper_drive(&zapper, &files, &theme, cx),
            ZapperPage::File(path) => {
                self.zapper_document(&zapper, &path, &files, &theme, window, cx)
            }
            ZapperPage::Section(ZapperTab::History)
            | ZapperPage::History
            | ZapperPage::PullRequest(_)
            | ZapperPage::Subagent(_)
            | ZapperPage::Diff(_) => self.zapper_history(&zapper, files, &theme, cx),
        };
        let body = div()
            .id("zapper-inspector-body")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .px(px(16.0))
            .py(px(16.0))
            .flex()
            .flex_col()
            .gap(px(18.0))
            .child(body)
            .into_any_element();
        self.zapper_inspector_frame(&theme, tab_bar, body)
    }

    /// The panel sits in the same inset card as a chat's right pane.
    fn zapper_inspector_frame(
        &self,
        theme: &Theme,
        tab_bar: AnyElement,
        body: AnyElement,
    ) -> AnyElement {
        let gutter = self.workspace_gutter();
        let card = div()
            .id("zapper-inspector")
            .debug_selector(|| "zapper-inspector".into())
            .size_full()
            .flex()
            .flex_col()
            .bg(theme.panel_bg())
            .overflow_hidden()
            // The content row spans the window; clear the title bar's controls.
            .pt(px(Theme::TITLEBAR_HEIGHT - gutter))
            .map(|card| {
                if gutter > 0.0 {
                    Self::inset_pane_card(card)
                } else {
                    card.border_l_1().border_color(theme.border)
                }
            })
            .child(tab_bar)
            .child(body);
        div()
            .w(px(ZAPPER_PANEL_WIDTH))
            .h_full()
            .flex_none()
            .map(|frame| Self::inset_pane_frame(frame, gutter))
            .child(card)
            .into_any_element()
    }

    fn zapper_issue_surface(
        &mut self,
        zapper: &Zapper,
        overview: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        use zuse_viewport::zapper_issues::{IssueEvent, IssuePanel, ThreadState};
        if self.zapper_ui.issues.is_none() {
            let panel = cx.new(|_| IssuePanel::new(zapper.clone(), overview));
            cx.subscribe(&panel, |this, _, event, cx| match event {
                IssueEvent::Command(action, revision) => {
                    if let Some(zapper) = this.selected_zapper(cx) {
                        let mut action = action.clone();
                        if let ZapperAction::Delegate { model, .. } = &mut action {
                            *model = this.zapper_thread_model(&zapper, cx);
                        }
                        this.zapper_ui.issue_command = true;
                        this.send_zapper_command(
                            zapper.id,
                            Some(*revision),
                            action,
                            ZapperAfter::Stay,
                            cx,
                        );
                    }
                }
                IssueEvent::OpenThread(id) => this.open_chat(id.clone(), cx),
                IssueEvent::OpenDocument(file) => {
                    this.show_zapper_page(ZapperPage::File(file.path.clone()), cx);
                }
                IssueEvent::OpenPullRequest(url) => {
                    this.show_zapper_page(ZapperPage::PullRequest(url.clone()), cx);
                }
                IssueEvent::NewThread => {
                    if let Some(zapper) = this.selected_zapper(cx) {
                        this.open_zapper_dialog(zapper, Kind::Thread(None), cx);
                    }
                }
                IssueEvent::ShowIssues => {
                    this.show_zapper_page(ZapperPage::Section(ZapperTab::Issues), cx);
                }
            })
            .detach();
            // The surface opens issues itself (a list row, a relation, a new
            // issue). Each one becomes a tab instead of replacing the list.
            cx.observe(&panel, |this, panel, cx| {
                let selected = panel.read(cx).selected_id().map(str::to_owned);
                if selected != this.zapper_ui.issue_followed {
                    this.zapper_ui.issue_followed = selected.clone();
                    if let Some(id) = selected
                        && this.zapper_ui.tabs.active_issue() != Some(id.as_str())
                    {
                        this.show_zapper_page(ZapperPage::Issue(id), cx);
                    }
                }
                cx.notify();
            })
            .detach();
            self.zapper_ui.issues = Some(panel);
        }
        let panel = self.zapper_ui.issues.as_ref().unwrap().clone();
        // The tab decides which issue the shared surface shows.
        let wanted = self.zapper_ui.tabs.active_issue().map(str::to_owned);
        if panel.read(cx).selected_id() != wanted.as_deref() {
            panel.update(cx, |p, cx| match wanted {
                Some(id) => p.show_issue(id, cx),
                None => p.show_list(cx),
            });
        }
        let states: std::collections::HashMap<String, ThreadState> = zapper
            .workers
            .iter()
            .map(|w| {
                let state = if self.zapper_thread_archived(&w.chat_id, cx) {
                    ThreadState::Archived
                } else {
                    match worker_phase(w, self.zapper_thread_phase(&w.chat_id, cx).0) {
                        ThreadPhase::Working => ThreadState::Working,
                        ThreadPhase::NeedsYou(_) => ThreadState::NeedsYou,
                        ThreadPhase::NotStarted | ThreadPhase::Queued | ThreadPhase::Done => {
                            ThreadState::Idle
                        }
                    }
                };
                (w.chat_id.clone(), state)
            })
            .collect();
        let user = self.state.read(cx).auth_user().map(|user| {
            user.name
                .as_deref()
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .unwrap_or(&user.email)
                .to_owned()
        });
        panel.update(cx, |p, cx| {
            p.thread_states(states, cx);
            p.user_name(user, cx);
            p.sync(zapper.clone(), overview, self.zapper_ui.pending, cx);
            p.files(self.zapper_ui.files.clone(), cx);
            p.thread_titles(
                self.state
                    .read(cx)
                    .chats
                    .iter()
                    .filter_map(|c| Some((c.id.clone(), c.title.clone()?)))
                    .collect(),
                cx,
            );
        });
        panel.into_any_element()
    }
    /// The Overview's header: face, name and state, the purpose, and anything
    /// waiting on the user. Open work follows from the shared issue surface.
    fn zapper_overview(
        &mut self,
        zapper: &Zapper,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        use zuse_protocol::SessionStatus as S;
        let working = zapper
            .workers
            .iter()
            .filter(|w| self.zapper_thread_status(&w.chat_id, cx) == Some(S::Working))
            .count();
        let (status_color, status) = match zapper.status {
            ZapperStatus::Active if working > 0 => (
                theme.success,
                if working == 1 {
                    "1 thread working".to_owned()
                } else {
                    format!("{working} threads working")
                },
            ),
            ZapperStatus::Active => (theme.success, "Active".to_owned()),
            ZapperStatus::Paused => (theme.warning, "Paused".to_owned()),
            ZapperStatus::Stopped => (theme.text_faint, "Stopped".to_owned()),
            ZapperStatus::Archived => (theme.text_faint, "Archived".to_owned()),
        };
        let toggle = if zapper.status == ZapperStatus::Active {
            ZapperStatus::Paused
        } else {
            ZapperStatus::Active
        };
        let (id, rev) = (zapper.id.clone(), zapper.revision);
        let hero = div()
            .flex()
            .items_center()
            .gap(px(12.0))
            .child(zapper_mascot(&self.zapper_face(zapper, cx), 40.0))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .child(
                        div()
                            .truncate()
                            .text_size(crate::typography::ui_rems(15.0))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(theme.text_strong)
                            .child(zapper.config.name.clone()),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.0))
                            .text_size(crate::typography::ui_rems(12.0))
                            .text_color(theme.text_muted)
                            .child(div().size(px(6.0)).rounded_full().bg(status_color))
                            .child(status),
                    ),
            )
            .when(
                matches!(zapper.status, ZapperStatus::Active | ZapperStatus::Paused),
                |d| {
                    d.child(
                        div()
                            .id("zapper-pause")
                            .role(gpui::Role::Button)
                            .h(px(26.0))
                            .px(px(10.0))
                            .flex()
                            .items_center()
                            .gap(px(6.0))
                            .rounded(px(7.0))
                            .border_1()
                            .border_color(theme.border)
                            .cursor_pointer()
                            .text_size(crate::typography::ui_rems(12.0))
                            .text_color(theme.text)
                            .hover(|d| d.bg(theme.element_hover))
                            .child(if toggle == ZapperStatus::Paused {
                                "Pause"
                            } else {
                                "Resume"
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.send_zapper_command(
                                    id.clone(),
                                    Some(rev),
                                    ZapperAction::SetStatus { status: toggle },
                                    ZapperAfter::Stay,
                                    cx,
                                )
                            })),
                    )
                },
            );
        let asks = needs_you_items(&zapper.issues, &zapper.decisions);
        let mut view = div().flex().flex_col().gap(px(14.0)).child(hero).when(
            !zapper.config.purpose.trim().is_empty(),
            |d| {
                d.child(
                    div()
                        .text_size(crate::typography::ui_rems(12.5))
                        .line_height(crate::typography::ui_rems(18.0))
                        .text_color(theme.text_muted)
                        .line_clamp(3)
                        .child(zapper.config.purpose.clone()),
                )
            },
        );
        if !asks.is_empty() {
            let mut needs = div()
                .flex()
                .flex_col()
                .gap(px(2.0))
                .p(px(6.0))
                .rounded(px(10.0))
                .border_1()
                .border_color(theme.warning.opacity(0.25))
                .bg(theme.warning.opacity(0.05))
                .child(
                    div()
                        .px(px(4.0))
                        .pb(px(2.0))
                        .text_size(crate::typography::ui_rems(11.5))
                        .text_color(theme.warning)
                        .child("Needs you"),
                );
            for item in asks {
                let (glyph, id) = match &item {
                    NeedsYouItem::Decision { id, .. } => {
                        (icons::INFO_CIRCLE, format!("zapper-decision-{id}"))
                    }
                    NeedsYouItem::Issue { id, .. } => {
                        (icons::STATUS_QUESTION, format!("zapper-asked-{id}"))
                    }
                };
                let z = zapper.clone();
                let decisions = zapper.decisions.clone();
                let title = item.title();
                let target = item.target();
                needs = needs.child(list_row(
                    id,
                    glyph,
                    theme.warning,
                    title,
                    Some("Answer".into()),
                    theme,
                    move |this, _, _, cx| match &target {
                        NeedsYouTarget::IssueTab(id) => {
                            this.show_zapper_page(ZapperPage::Issue(id.clone()), cx)
                        }
                        NeedsYouTarget::DecisionForm(id) => {
                            if let Some(decision) = decisions.iter().find(|d| &d.id == id) {
                                this.open_zapper_dialog(
                                    z.clone(),
                                    Kind::Decision(decision.clone()),
                                    cx,
                                );
                            }
                        }
                    },
                    cx,
                ));
            }
            view = view.child(needs);
        }
        view.into_any_element()
    }

    /// The repositories this Zapper works in, as a quiet line of chips.
    fn zapper_repositories(
        &mut self,
        zapper: &Zapper,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let names: Vec<(String, String)> = {
            let state = self.state.read(cx);
            zapper
                .config
                .repositories
                .iter()
                .filter_map(|repository| {
                    let space = state.spaces.iter().find(|s| s.id == repository.space_id)?;
                    let name = space
                        .name
                        .clone()
                        .or_else(|| space.path.rsplit('/').next().map(str::to_owned))?;
                    Some((space.id.clone(), name))
                })
                .collect()
        };
        let chip = |id: SharedString| {
            div()
                .id(id)
                .h(px(24.0))
                .px(px(8.0))
                .flex()
                .items_center()
                .gap(px(6.0))
                .rounded(px(6.0))
                .bg(theme.element_hover.opacity(0.6))
                .text_size(crate::typography::ui_rems(12.0))
                .text_color(theme.text)
        };
        let mut chips = div().flex().flex_wrap().gap(px(6.0));
        if names.is_empty() {
            let z = zapper.clone();
            chips = chips.child(
                chip("zapper-add-repositories".into())
                    .role(gpui::Role::Button)
                    .cursor_pointer()
                    .text_color(theme.text_muted)
                    .hover(|d| d.text_color(theme.text))
                    .child(icon(icons::PLUS).size(px(11.0)))
                    .child("Choose repositories")
                    .on_click(
                        cx.listener(move |this, _, _, cx| this.open_zapper_editor(z.clone(), cx)),
                    ),
            );
        }
        for (id, name) in names {
            chips = chips.child(
                chip(SharedString::from(format!("zapper-overview-repo-{id}")))
                    .child(
                        icon(icons::FOLDER)
                            .size(px(12.0))
                            .text_color(theme.text_muted),
                    )
                    .child(name),
            );
        }
        div()
            .flex()
            .flex_col()
            .gap(px(6.0))
            .child(
                div()
                    .px(px(2.0))
                    .text_size(crate::typography::ui_rems(12.0))
                    .text_color(theme.text_muted)
                    .child("Repositories"),
            )
            .child(chips)
            .into_any_element()
    }

    pub(super) fn zapper_manage(
        &mut self,
        zapper: &Zapper,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut manage = div()
            .flex()
            .flex_col()
            .gap(px(2.0))
            .child(section("Manage", theme));
        for (key, glyph, label, kind) in [
            (
                "zapper-fresh",
                icons::RESTART,
                "Start a fresh conversation",
                0,
            ),
            ("zapper-stop", icons::STOP, "Stop Zapper", 1),
            (
                "zapper-archive",
                icons::ARCHIVE_MINIMALISTIC,
                "Archive Zapper and threads",
                2,
            ),
        ] {
            let z = zapper.clone();
            manage = manage.child(list_row(
                key,
                glyph,
                theme.text_muted,
                label.to_owned(),
                None,
                theme,
                move |this, _, _, cx| {
                    let (action, after) = match kind {
                        0 => (
                            ZapperAction::FreshConversation,
                            ZapperAfter::OpenCoordinator,
                        ),
                        1 => (
                            ZapperAction::SetStatus {
                                status: ZapperStatus::Stopped,
                            },
                            ZapperAfter::LeavePage,
                        ),
                        _ => (
                            ZapperAction::SetStatus {
                                status: ZapperStatus::Archived,
                            },
                            ZapperAfter::LeavePage,
                        ),
                    };
                    this.send_zapper_command(z.id.clone(), Some(z.revision), action, after, cx)
                },
                cx,
            ));
        }
        let z = zapper.clone();
        if self.zapper_ui.confirm_delete {
            manage = manage
                .child(muted(
                    "Delete this Zapper? Its threads and documents are kept.",
                    theme,
                ))
                .child(list_row(
                    "zapper-delete-confirm",
                    icons::TRASH_BIN_MINIMALISTIC,
                    theme.danger,
                    "Delete Zapper".to_owned(),
                    None,
                    theme,
                    move |this, _, _, cx| {
                        this.zapper_ui.confirm_delete = false;
                        this.send_zapper_command(
                            z.id.clone(),
                            Some(z.revision),
                            ZapperAction::Delete,
                            ZapperAfter::LeavePage,
                            cx,
                        );
                    },
                    cx,
                ));
        } else {
            manage = manage.child(list_row(
                "zapper-delete",
                icons::TRASH_BIN_MINIMALISTIC,
                theme.danger,
                "Delete…".to_owned(),
                None,
                theme,
                |this, _, _, cx| {
                    this.zapper_ui.confirm_delete = true;
                    cx.notify();
                },
                cx,
            ));
        }
        manage.into_any_element()
    }

    fn zapper_threads(
        &mut self,
        zapper: &Zapper,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let workers = ordered_zapper_threads(
            zapper
                .workers
                .iter()
                .filter(|w| !self.zapper_thread_archived(&w.chat_id, cx)),
            |w| {
                use zuse_protocol::SessionStatus as S;
                match self.zapper_thread_status(&w.chat_id, cx) {
                    Some(S::Working) => ThreadRank::Working,
                    Some(S::AwaitingInput | S::Errored) => ThreadRank::NeedsYou,
                    Some(S::Idle) | None => ThreadRank::Idle,
                }
            },
        );
        let new_zapper = zapper.clone();
        let mut threads = div().flex().flex_col().gap(px(2.0)).child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .child(section(format!("Threads · {}", workers.len()), theme))
                .child(action(
                    "zapper-new-worker",
                    "New thread",
                    theme,
                    move |this, _, _, cx| {
                        this.open_zapper_dialog(new_zapper.clone(), Kind::Thread(None), cx)
                    },
                    cx,
                )),
        );
        if workers.is_empty() {
            threads = threads.child(muted(
                "Threads this Zapper starts or adopts appear here.",
                theme,
            ));
        }
        for worker in workers {
            let id = worker.chat_id.clone();
            use zuse_protocol::SessionStatus as S;
            let status = match self.zapper_thread_status(&id, cx) {
                Some(S::Working) => "Working",
                Some(S::AwaitingInput) => "Waiting for input",
                Some(S::Errored) => "Failed",
                Some(S::Idle) => "Idle",
                None => "Not running",
            };
            let dot = self.zapper_status_color(&id, theme, cx);
            let open = id.clone();
            let row_group = SharedString::from(format!("zapper-thread-row-{id}"));
            let mut actions = div()
                .absolute()
                .right(px(4.0))
                .top(px(0.0))
                .bottom(px(0.0))
                .flex()
                .items_center()
                .gap(px(1.0))
                .invisible()
                .group_hover(row_group.clone(), |d| d.visible());
            let z = zapper.clone();
            let message = id.clone();
            actions = actions.child(thread_action(
                format!("zapper-message-{id}"),
                icons::CHAT_ROUND_LINE,
                "Send a message",
                theme,
                move |this, _, _, cx| {
                    this.open_zapper_dialog(z.clone(), Kind::Thread(Some(message.clone())), cx)
                },
                cx,
            ));
            for (label, glyph, kind) in [
                ("Stop", icons::STOP, 0),
                ("Release from Zapper", icons::ARROW_UP_RIGHT, 2),
                ("Archive", icons::ARCHIVE_MINIMALISTIC, 1),
            ] {
                let chat_id = id.clone();
                let z = zapper.clone();
                actions = actions.child(thread_action(
                    format!("zapper-worker-{kind}-{id}"),
                    glyph,
                    label,
                    theme,
                    move |this, _, _, cx| {
                        let chat_id = chat_id.clone();
                        if kind == 1 {
                            this.archive_zapper_thread(&z, chat_id, cx);
                            return;
                        }
                        let action = if kind == 0 {
                            ZapperAction::StopWorker { chat_id }
                        } else {
                            ZapperAction::ReleaseWorker { chat_id }
                        };
                        this.send_zapper_command(
                            z.id.clone(),
                            Some(z.revision),
                            action,
                            ZapperAfter::Stay,
                            cx,
                        );
                    },
                    cx,
                ));
            }
            threads = threads.child(
                div()
                    .id(SharedString::from(format!("zapper-open-{id}")))
                    .group(row_group)
                    .role(gpui::Role::Button)
                    .aria_label(SharedString::from(worker.objective.clone()))
                    .relative()
                    .mx(px(-8.0))
                    .flex()
                    .items_center()
                    .gap(px(10.0))
                    .px(px(8.0))
                    .py(px(5.0))
                    .rounded(px(6.0))
                    .cursor_pointer()
                    .hover(|d| d.bg(theme.element_hover))
                    .child(div().size(px(7.0)).flex_none().rounded_full().bg(dot))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .truncate()
                                    .text_size(crate::typography::ui_rems(13.0))
                                    .text_color(theme.text)
                                    .child(worker.objective.clone()),
                            )
                            .child(
                                div()
                                    .text_size(crate::typography::ui_rems(11.0))
                                    .text_color(theme.text_faint)
                                    .child(status),
                            ),
                    )
                    .child(actions)
                    .on_click(cx.listener(move |this, _, _, cx| this.open_chat(open.clone(), cx))),
            );
        }
        let mut view = div().flex().flex_col().gap(px(14.0)).child(threads);
        if let Some(finished) = self.zapper_finished_threads(zapper, theme, cx) {
            view = view.child(finished);
        }
        let mut coordinator = div()
            .flex()
            .flex_col()
            .gap(px(2.0))
            .child(section("Coordinator conversations", theme));
        let current = zapper.coordinator_chat_id.clone();
        coordinator = coordinator.child(list_row(
            "zapper-current-conversation",
            icons::CHAT_ROUND_LINE,
            theme.text_muted,
            "Current conversation".to_owned(),
            None,
            theme,
            move |this, _, _, cx| {
                this.open_chat(current.clone(), cx);
            },
            cx,
        ));
        for (index, id) in zapper.previous_coordinator_chats.iter().enumerate().rev() {
            let id = id.clone();
            coordinator = coordinator.child(list_row(
                format!("zapper-history-{id}"),
                icons::HISTORY,
                theme.text_faint,
                format!("Conversation {}", index + 1),
                None,
                theme,
                move |this, _, _, cx| {
                    this.open_chat(id.clone(), cx);
                },
                cx,
            ));
        }
        view = view.child(coordinator);
        view.into_any_element()
    }

    /// Threads this Zapper started and later released. They stay out of
    /// project lists, so this collapsed group keeps them a click away.
    fn zapper_finished_threads(
        &mut self,
        zapper: &Zapper,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let finished: Vec<String> = finished_zapper_threads(zapper)
            .into_iter()
            .filter(|id| !self.zapper_thread_archived(id, cx))
            .collect();
        if finished.is_empty() {
            return None;
        }
        let open = self.zapper_ui.finished_open;
        let mut group = div().flex().flex_col().gap(px(2.0)).child(
            div()
                .id("zapper-finished-threads")
                .role(gpui::Role::Button)
                .aria_label(SharedString::from(format!(
                    "Finished threads, {}",
                    if open { "expanded" } else { "collapsed" }
                )))
                .flex()
                .items_center()
                .gap(px(4.0))
                .cursor_pointer()
                .child(section(format!("Finished · {}", finished.len()), theme))
                .child(
                    icon(if open {
                        icons::ALT_ARROW_DOWN
                    } else {
                        icons::ALT_ARROW_RIGHT
                    })
                    .size(px(11.0))
                    .mb(px(4.0))
                    .text_color(theme.text_faint),
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.zapper_ui.finished_open = !this.zapper_ui.finished_open;
                    cx.notify();
                })),
        );
        if !open {
            return Some(group.into_any_element());
        }
        for id in finished {
            let title = self
                .state
                .read(cx)
                .chats
                .iter()
                .find(|c| c.id == id)
                .and_then(|c| c.title.clone())
                .filter(|t| !t.trim().is_empty())
                .unwrap_or_else(|| id.clone());
            let code = zapper
                .issues
                .iter()
                .find(|i| i.threads.contains(&id))
                .map(|i| i.code());
            let open = id.clone();
            group = group.child(list_row(
                format!("zapper-finished-{id}"),
                icons::CHAT_ROUND_LINE,
                theme.text_faint,
                title,
                code,
                theme,
                move |this, _, _, cx| this.open_chat(open.clone(), cx),
                cx,
            ));
        }
        Some(group.into_any_element())
    }

    /// The folder new files and folders go in: the selected folder, or the
    /// selected file's folder, as a path prefix.
    fn zapper_tree_prefix(&self) -> String {
        let Some(selected) = &self.zapper_ui.tree_selected else {
            return String::new();
        };
        let folder = match selected.split_once(':') {
            Some(("d", path)) => path,
            Some((_, path)) => path.rsplit_once('/').map_or("", |(parent, _)| parent),
            None => "",
        };
        if folder.is_empty() {
            String::new()
        } else {
            format!("{folder}/")
        }
    }

    /// Drive: the Zapper's files and folders as a tree, like the workspace
    /// explorer. Folders toggle, files open in their own tab.
    fn zapper_drive(
        &mut self,
        zapper: &Zapper,
        files: &[ZapperFile],
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        use zuse_protocol::presentation::zapper_files::{initially_open, tree_rows};
        let loaded = self.zapper_ui.files_zapper.as_deref() == Some(zapper.id.as_str());
        if loaded && self.zapper_ui.tree_zapper.as_deref() != Some(zapper.id.as_str()) {
            self.zapper_ui.tree_zapper = Some(zapper.id.clone());
            self.zapper_ui.tree_open = initially_open(files, &zapper.folders);
            self.zapper_ui.tree_selected = None;
        }
        self.zapper_ui.tree_folders = zapper.folders.clone();
        let focus = self
            .zapper_ui
            .tree_focus
            .get_or_insert_with(|| cx.focus_handle().tab_stop(true))
            .clone();
        let rows = tree_rows(files, &zapper.folders, &self.zapper_ui.tree_open);
        let mut view = div()
            .flex()
            .flex_col()
            .child(section("Files", theme))
            .child(
                div()
                    .id("zapper-files-tree")
                    .role(gpui::Role::Tree)
                    .aria_label("Zapper files")
                    .track_focus(&focus)
                    .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                        this.zapper_tree_key(event, cx)
                    }))
                    .mx(px(-8.0))
                    .flex()
                    .flex_col()
                    .children(
                        rows.iter()
                            .map(|row| self.zapper_file_row(zapper, row, files, theme, cx)),
                    ),
            );
        if !loaded && files.is_empty() {
            view = view.child(muted("Loading files…", theme));
        } else if !rows.iter().any(|r| r.folder || !r.reserved) {
            view = view.child(
                div()
                    .pt(px(10.0))
                    .text_size(crate::typography::ui_rems(12.0))
                    .text_color(theme.text_faint)
                    .child("Add folders and documents for the coordinator and its threads to open when they need them. Files you add stay out of their prompts until opened."),
            );
        }
        view.into_any_element()
    }

    /// The file tree's keys, like the workspace explorer: Up and Down move
    /// the selection, Right opens a folder or steps into it, Left closes it
    /// or steps out, Enter opens a file or toggles a folder, F2 renames or
    /// moves and Delete deletes, both behind their dialogs.
    fn zapper_tree_key(&mut self, event: &gpui::KeyDownEvent, cx: &mut Context<Self>) {
        use zuse_protocol::presentation::zapper_files::{TreeKey, tree_key, tree_rows};
        let Some(zapper) = self.selected_zapper(cx) else {
            return;
        };
        let files = self.zapper_ui.files.clone();
        let rows = tree_rows(&files, &zapper.folders, &self.zapper_ui.tree_open);
        let Some(action) = tree_key(
            &rows,
            self.zapper_ui.tree_selected.as_deref(),
            event.keystroke.key.as_str(),
        ) else {
            return;
        };
        cx.stop_propagation();
        match action {
            TreeKey::Select(key) => self.zapper_ui.tree_selected = Some(key),
            TreeKey::Expand(path) => {
                self.zapper_ui.tree_open.insert(path);
            }
            TreeKey::Collapse(path) => {
                self.zapper_ui.tree_open.remove(&path);
            }
            TreeKey::Open(path) => return self.show_zapper_page(ZapperPage::File(path), cx),
            TreeKey::Move(row) | TreeKey::Delete(row) => {
                let delete = matches!(event.keystroke.key.as_str(), "delete" | "backspace");
                let kind = super::zapper_forms::file_action(&row, &files, &zapper.folders, delete);
                return self.open_zapper_dialog(zapper, kind, cx);
            }
        }
        cx.notify();
    }

    fn zapper_file_row(
        &self,
        zapper: &Zapper,
        row: &zuse_protocol::presentation::zapper_files::FileRow,
        files: &[ZapperFile],
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        use crate::file_icons::FileIconIdentity;
        use crate::files::tree::{TREE_INDENT, TREE_ROW_HEIGHT, with_indent_guides};
        let key = row.key();
        let selected = self.zapper_ui.tree_selected.as_deref() == Some(key.as_str());
        let path = row.path.clone();
        let folder = row.folder;
        let trailing = |id: &str, glyph: &'static str, tip: &'static str| {
            div()
                .id(SharedString::from(format!("{id}-{}", row.path)))
                .role(gpui::Role::Button)
                .aria_label(SharedString::from(format!("{tip} {}", row.path)))
                .tip(tip)
                .size(px(20.0))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(5.0))
                .cursor_pointer()
                .hover(|d| d.bg(theme.element_hover))
                .child(icon(glyph).size(px(12.0)).text_color(theme.text_muted))
        };
        let actions = (selected && !row.reserved).then(|| {
            let (move_zapper, delete_zapper) = (zapper.clone(), zapper.clone());
            let (move_kind, delete_kind) = (
                super::zapper_forms::file_action(row, files, &zapper.folders, false),
                super::zapper_forms::file_action(row, files, &zapper.folders, true),
            );
            div()
                .ml_auto()
                .flex()
                .gap(px(2.0))
                .child(
                    trailing("zapper-file-move", icons::PEN, "Rename or move").on_click(
                        cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.open_zapper_dialog(move_zapper.clone(), move_kind.clone(), cx)
                        }),
                    ),
                )
                .child(
                    trailing(
                        "zapper-file-delete",
                        icons::TRASH_BIN_MINIMALISTIC,
                        "Delete",
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.open_zapper_dialog(delete_zapper.clone(), delete_kind.clone(), cx)
                    })),
                )
        });
        let content = div()
            .id(SharedString::from(format!("zapper-file-row-{key}")))
            .role(gpui::Role::TreeItem)
            .aria_label(SharedString::from(row.path.clone()))
            .aria_selected(selected)
            .when(folder, |d| d.aria_expanded(row.expanded))
            .debug_selector({
                let key = key.clone();
                move || format!("zapper-file:{key}")
            })
            .h(px(TREE_ROW_HEIGHT))
            .w_full()
            .pl(px(8.0 + row.depth as f32 * TREE_INDENT))
            .pr(px(6.0))
            .flex()
            .items_center()
            .gap(px(6.0))
            .rounded(px(6.0))
            .cursor_pointer()
            .when(selected, |d| d.bg(crate::theme::wash(0.12)))
            .when(!selected, |d| d.hover(|s| s.bg(crate::theme::wash(0.055))))
            .child(
                div()
                    .size(px(14.0))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .when(folder, |d| {
                        d.child(
                            icon(if row.expanded {
                                icons::ALT_ARROW_DOWN
                            } else {
                                icons::ALT_ARROW_RIGHT
                            })
                            .size(px(11.0))
                            .text_color(theme.text_faint),
                        )
                    }),
            )
            .child(if folder {
                icon(if row.expanded {
                    icons::FOLDER_OPEN
                } else {
                    icons::FOLDER
                })
                .size(px(14.0))
                .flex_none()
                .text_color(theme.text_muted)
                .into_any_element()
            } else {
                crate::file_icons::icon(FileIconIdentity::file(&row.name), theme.appearance)
                    .size(px(14.0))
                    .when(row.version.is_none(), |d| d.opacity(0.55))
                    .into_any_element()
            })
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .text_size(crate::typography::ui_rems(12.5))
                    .text_color(if selected {
                        theme.text_strong
                    } else if folder && !row.expanded || row.version.is_none() && !folder {
                        theme.text_muted
                    } else {
                        theme.text
                    })
                    .child(row.name.clone()),
            )
            .children(
                zuse_protocol::presentation::zapper_files::reserved_role(&row.path)
                    .filter(|_| !folder && row.depth == 0)
                    .map(|role| {
                        div()
                            .flex_none()
                            .text_size(crate::typography::ui_rems(11.0))
                            .text_color(theme.text_faint)
                            .child(if row.version.is_none() {
                                format!("{role} · not written")
                            } else {
                                role.to_owned()
                            })
                    }),
            )
            .when(row.conflict, |d| {
                d.child(
                    div()
                        .flex_none()
                        .text_size(crate::typography::ui_rems(11.0))
                        .text_color(theme.warning)
                        .child("conflicts with folder"),
                )
            })
            .children(actions)
            .on_click(cx.listener(move |this, _, window, cx| {
                if let Some(focus) = &this.zapper_ui.tree_focus {
                    focus.focus(window, cx);
                }
                this.zapper_ui.tree_selected = Some(key.clone());
                if folder {
                    if !this.zapper_ui.tree_open.remove(&path) {
                        this.zapper_ui.tree_open.insert(path.clone());
                    }
                    cx.notify();
                } else {
                    this.show_zapper_page(ZapperPage::File(path.clone()), cx);
                }
            }))
            .into_any_element();
        with_indent_guides(content, row.depth, TREE_ROW_HEIGHT, theme)
    }

    /// A Drive document in its own tab, rendered as markdown. Edit opens the
    /// document editor.
    fn zapper_document(
        &mut self,
        zapper: &Zapper,
        path: &str,
        files: &[ZapperFile],
        theme: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        use zuse_protocol::presentation::zapper_files::{reserved_about, reserved_role};
        let about = reserved_about(path).map(|about| muted(about, theme));
        let Some(file) = latest_file(files, path).cloned() else {
            if self.zapper_ui.loading || self.zapper_history_view() {
                return muted("Loading document…", theme).into_any_element();
            }
            let Some(about) = about else {
                return muted("This file was moved or deleted.", theme).into_any_element();
            };
            // A reserved file an older Zapper never wrote: write it here.
            let (z, path) = (zapper.clone(), path.to_owned());
            return div()
                .flex()
                .flex_col()
                .gap(px(10.0))
                .child(about)
                .child(muted("Not written yet.", theme))
                .child(action(
                    "zapper-document-write",
                    "Write",
                    theme,
                    move |this, _, _, cx| {
                        let file = ZapperFile {
                            path: path.clone(),
                            ..Default::default()
                        };
                        this.open_zapper_dialog(z.clone(), Kind::File(file), cx)
                    },
                    cx,
                ))
                .into_any_element();
        };
        let tree = match self.zapper_ui.documents.get(path) {
            Some((version, tree)) if *version == file.version => tree.clone(),
            _ => {
                let tree = std::sync::Arc::new(crate::markdown::parse_full(&file.content));
                self.zapper_ui
                    .documents
                    .insert(path.to_owned(), (file.version, tree.clone()));
                tree
            }
        };
        let options = crate::markdown::render::RenderOptions::settled(
            format!("zapper-document-{path}").into(),
        );
        let z = zapper.clone();
        let header = div()
            .flex()
            .items_center()
            .gap(px(8.0))
            .child(
                icon(icons::DOCUMENT)
                    .size(px(14.0))
                    .flex_none()
                    .text_color(theme.text_muted),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(crate::typography::ui_rems(13.0))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(theme.text_strong)
                    .child(file.path.clone()),
            )
            .children(reserved_role(path).map(|role| {
                div()
                    .flex_none()
                    .text_size(crate::typography::ui_rems(11.0))
                    .text_color(theme.text_muted)
                    .child(role)
            }))
            .child(
                div()
                    .text_size(crate::typography::ui_rems(11.0))
                    .text_color(theme.text_faint)
                    .child(format!("v{}", file.version)),
            )
            .child(action(
                "zapper-document-edit",
                "Edit",
                theme,
                move |this, _, _, cx| {
                    this.open_zapper_dialog(z.clone(), Kind::File(file.clone()), cx)
                },
                cx,
            ));
        div()
            .flex()
            .flex_col()
            .gap(px(14.0))
            .child(header)
            .children(about)
            .child(
                div()
                    .min_w_0()
                    .text_size(crate::typography::ui_rems(13.0))
                    .child(crate::markdown::render::render_tree(
                        &tree,
                        &options,
                        theme,
                        window,
                        &|_| None,
                    )),
            )
            .into_any_element()
    }

    fn zapper_history(
        &mut self,
        zapper: &Zapper,
        files: Vec<ZapperFile>,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let latest = files.iter().fold(
            std::collections::HashMap::<String, u32>::new(),
            |mut versions, file| {
                versions
                    .entry(file.path.clone())
                    .and_modify(|v| *v = (*v).max(file.version))
                    .or_insert(file.version);
                versions
            },
        );
        let mut view = div()
            .flex()
            .flex_col()
            .gap(px(2.0))
            .child(section("Document versions", theme));
        if files.is_empty() {
            view = view.child(muted(
                if self.zapper_ui.loading {
                    "Loading history…"
                } else {
                    "No versions yet."
                },
                theme,
            ));
        }
        let mut files = files;
        files.sort_by(|a, b| a.path.cmp(&b.path).then(b.version.cmp(&a.version)));
        for file in files {
            let current = *latest.get(&file.path).unwrap_or(&file.version);
            let z = zapper.clone();
            view = view.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .px(px(6.0))
                    .py(px(5.0))
                    .child(
                        icon(icons::DOCUMENT)
                            .size(px(13.0))
                            .text_color(theme.text_faint),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(crate::typography::ui_rems(12.5))
                            .text_color(theme.text)
                            .child(file.path.clone()),
                    )
                    .child(
                        div()
                            .text_size(crate::typography::ui_rems(11.0))
                            .text_color(theme.text_faint)
                            .child(if current == file.version {
                                format!("v{} · current", file.version)
                            } else {
                                format!("v{}", file.version)
                            }),
                    )
                    .when(current > file.version, |d| {
                        d.child(action(
                            format!("zapper-restore-{}-{}", file.path, file.version),
                            "Restore",
                            theme,
                            move |this, _, _, cx| {
                                this.send_zapper_command(
                                    z.id.clone(),
                                    Some(z.revision),
                                    ZapperAction::RestoreFile {
                                        path: file.path.clone(),
                                        version: file.version,
                                        expected_version: current,
                                    },
                                    ZapperAfter::Stay,
                                    cx,
                                )
                            },
                            cx,
                        ))
                    }),
            );
        }
        view.into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(path: &str, version: u32, content: &str) -> ZapperFile {
        ZapperFile {
            path: path.into(),
            version,
            content: content.into(),
            deleted: false,
        }
    }

    fn queued_zapper() -> Zapper {
        let worker = |chat: &str, queue_position| ZapperWorker {
            chat_id: chat.into(),
            device_id: "local".into(),
            space_id: "repo".into(),
            objective: format!("Objective of {chat}"),
            followups: 0,
            queue_position,
        };
        serde_json::from_value(serde_json::json!({
            "id": "z", "revision": 1,
            "config": {
                "name": "Z", "purpose": "", "homeDeviceId": "local", "repositories": [],
                "coordinatorModel": {"harness": "mock", "model": null},
                "workerDeviceId": "local", "maxConcurrentWorkers": 1, "maxFollowups": 3
            },
            "status": "active", "coordinatorChatId": "zapper-z-0",
            "previousCoordinatorChats": [], "decisions": [],
            "workers": [
                worker("running", None),
                worker("queued", Some(2)),
                worker("starting", None),
            ],
        }))
        .unwrap()
    }

    #[test]
    fn queued_threads_read_queued_in_the_sidebar() {
        use zuse_protocol::ChatIndicator as C;
        let zapper = queued_zapper();
        let phase = |chat: &str, chat_phase| {
            let worker = zapper.workers.iter().find(|w| w.chat_id == chat).unwrap();
            worker_phase(worker, chat_phase)
        };
        // No chat yet reads as NotStarted; the queue says why.
        assert!(matches!(
            phase("queued", ThreadPhase::NotStarted),
            ThreadPhase::Queued
        ));
        assert!(matches!(
            phase("running", thread_phase(C::Working, true)),
            ThreadPhase::Working
        ));
        assert!(matches!(
            phase("starting", ThreadPhase::NotStarted),
            ThreadPhase::NotStarted
        ));
    }

    #[test]
    fn threads_without_a_conversation_get_a_placeholder_not_an_empty_pane() {
        let zappers = vec![queued_zapper()];
        let exists = |chat: &str| chat == "running" || chat == "zapper-z-0" || chat == "other";
        assert_eq!(
            thread_placeholder(&zappers, exists, "queued"),
            Some(ThreadPlaceholder::Queued {
                objective: "Objective of queued".into(),
                position: 2,
            })
        );
        // Delegated, its chat not reported yet: still never a blank pane.
        assert_eq!(
            thread_placeholder(&zappers, exists, "starting"),
            Some(ThreadPlaceholder::Starting {
                objective: "Objective of starting".into(),
            })
        );
        // Real conversations, and chats that aren't Zapper threads, render as usual.
        assert_eq!(thread_placeholder(&zappers, exists, "running"), None);
        assert_eq!(thread_placeholder(&zappers, exists, "zapper-z-0"), None);
        assert_eq!(thread_placeholder(&zappers, exists, "other"), None);
    }

    #[test]
    fn finished_threads_are_done_but_unstarted_ones_are_not() {
        use zuse_protocol::ChatIndicator as C;
        assert!(matches!(thread_phase(C::Idle, true), ThreadPhase::Done));
        assert!(matches!(
            thread_phase(C::Completed, true),
            ThreadPhase::Done
        ));
        assert!(matches!(
            thread_phase(C::Idle, false),
            ThreadPhase::NotStarted
        ));
        assert!(matches!(
            thread_phase(C::Working, false),
            ThreadPhase::Working
        ));
        assert!(matches!(
            thread_phase(C::Errored, true),
            ThreadPhase::NeedsYou(C::Errored)
        ));
    }

    #[test]
    fn the_expanded_section_fits_every_label() {
        let width = section_width(&ZAPPER_SECTIONS);
        for tab in ZAPPER_SECTIONS {
            assert!(width >= 39.0 + tab.label_width());
        }
    }

    #[test]
    fn document_tabs_show_the_newest_version() {
        let history = [
            file("NOTES.md", 1, "old"),
            file("NOTES.md", 3, "new"),
            file("NOTES.md", 2, "mid"),
        ];
        assert_eq!(latest_file(&history, "NOTES.md").unwrap().content, "new");
        assert!(latest_file(&history, "DIGEST.md").is_none());
        assert_eq!(
            pull_request_label("https://github.com/acme/app/pull/42"),
            "#42"
        );
    }
}

/// Native visual QA of the Zapper panel with seeded documents.
#[cfg(feature = "appshots-fixture")]
impl Shell {
    /// Focuses Drive's tree, so key presses the window receives reach it.
    pub fn fixture_zapper_tree_focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(focus) = self.zapper_ui.tree_focus.clone() {
            focus.focus(window, cx);
        }
    }

    /// Keep a seeded pull request tab offline once its page exists.
    /// A `-menu` step also opens Merge's menu.
    pub fn fixture_zapper_settle(&mut self, step: &str, cx: &mut Context<Self>) {
        if let Some((_, page)) = &self.zapper_ui.pull_request_page {
            let menu = step.ends_with("-menu");
            page.update(cx, |page, cx| page.fixture_offline(menu, cx));
        }
        cx.notify();
    }

    pub fn fixture_zapper_panel(
        &mut self,
        files: Vec<ZapperFile>,
        step: &str,
        cx: &mut Context<Self>,
    ) {
        let Some(zapper) = self.selected_zapper(cx) else {
            return;
        };
        self.zapper_ui.files = files;
        self.zapper_ui.files_zapper = Some(zapper.id.clone());
        self.zapper_ui.loaded =
            Some((zapper.id.clone(), zapper.revision, zapper.activity_revision));
        self.zapper_ui.dialog = None;
        self.close_zapper_overflow(cx);
        let issue = zapper
            .issues
            .first()
            .map(|i| ZapperPage::Issue(i.id.clone()));
        let file = self
            .zapper_ui
            .files
            .iter()
            .find(|f| is_document(f))
            .map(|f| ZapperPage::File(f.path.clone()));
        // Coordinator events: the card must not list them.
        self.zapper_ui.events = [
            (
                "e1",
                "thread",
                "Worker was interrupted: rebuild the base image",
            ),
            ("e2", "issueChanged", "ZAP-9 Linked pull request #280"),
            ("e3", "thread", "ZAP-8 Thread report: tests pass"),
        ]
        .into_iter()
        .map(|(id, kind, summary)| ZapperEvent {
            id: id.into(),
            zapper_id: zapper.id.clone(),
            worker_chat_id: if kind == "issueChanged" {
                String::new()
            } else {
                "zapper-worker-1".into()
            },
            kind: kind.into(),
            summary: summary.into(),
            acknowledged: false,
        })
        .collect();
        match step {
            "overview" => self.show_zapper_page(ZapperPage::Section(ZapperTab::Overview), cx),
            "issue" => {
                self.show_zapper_page(ZapperPage::Section(ZapperTab::Issues), cx);
                if let Some(page) = issue {
                    self.show_zapper_page(page, cx);
                }
            }
            "drive" => self.show_zapper_page(ZapperPage::Section(ZapperTab::Drive), cx),
            // `drive-nested`, `drive-delete`, `drive-move`, `drive-folder`,
            // `drive-empty`: the file tree opened, with a dialog for the action.
            step if step.starts_with("drive-") => {
                use zuse_protocol::{
                    presentation::zapper_files::tree_rows, zappers::RESERVED_FILES,
                };
                if step == "drive-empty" {
                    self.zapper_ui
                        .files
                        .retain(|f| RESERVED_FILES.contains(&f.path.as_str()));
                    self.state.update(cx, |state, _| {
                        if let Some(z) = state.zappers.iter_mut().find(|z| z.id == zapper.id) {
                            z.folders.clear();
                        }
                    });
                }
                let folders = if step == "drive-empty" {
                    vec![]
                } else {
                    zapper.folders.clone()
                };
                self.zapper_ui.tree_zapper = Some(zapper.id.clone());
                self.zapper_ui.tree_open = ["handoffs", "handoffs/design-references", "reports"]
                    .map(String::from)
                    .into();
                self.zapper_ui.tree_selected = Some("f:NOTES.md".into());
                self.show_zapper_page(ZapperPage::Section(ZapperTab::Drive), cx);
                let files = self.zapper_ui.files.clone();
                let row = |path: &str| {
                    tree_rows(&files, &folders, &self.zapper_ui.tree_open)
                        .into_iter()
                        .find(|r| r.path == path)
                };
                if step == "drive-keyboard" {
                    self.zapper_ui.tree_selected =
                        Some("f:handoffs/design-references/sidebar.md".into());
                }
                if step == "drive-conflict" {
                    // What an older engine leaves: a file over a folder.
                    self.zapper_ui.files.push(ZapperFile {
                        path: "handoffs".into(),
                        version: 1,
                        content: "Written by an older engine".into(),
                        deleted: false,
                    });
                    self.zapper_ui.tree_selected = Some("f:handoffs".into());
                }
                let kind = match step {
                    "drive-delete" => row("handoffs")
                        .map(|r| super::zapper_forms::file_action(&r, &files, &folders, true)),
                    "drive-move" => row("reports")
                        .map(|r| super::zapper_forms::file_action(&r, &files, &folders, false)),
                    "drive-folder" => Some(Kind::Folder("handoffs/".into())),
                    _ => None,
                };
                if let Some(kind) = kind {
                    self.open_zapper_dialog(zapper.clone(), kind, cx);
                }
            }
            "file" => {
                if let Some(page) = file {
                    self.show_zapper_page(page, cx);
                }
            }
            // `pr-ready`, `pr-blocked`, `pr-draft`: the PR tab on seeded,
            // offline details linked from the second issue.
            step if step.starts_with("pr-") => {
                let url = "https://github.com/zuse-fixture/zapper/pull/42".to_owned();
                crate::pull_request_detail::fixture_seed_zapper_pull_request(
                    &self.pull_request_cache,
                    Some(zapper.config.home_device_id.clone()),
                    &url,
                    step["pr-".len()..].trim_end_matches("-menu"),
                );
                self.zapper_ui.pull_request_page = None;
                self.zapper_ui.pull_request_subscription = None;
                let link = ZapperIssuePullRequest {
                    url: url.clone(),
                    title: "Zapper panel: closable tabs".into(),
                };
                self.state.update(cx, |state, _| {
                    if let Some(issue) = state
                        .zappers
                        .iter_mut()
                        .find(|z| z.id == zapper.id)
                        .and_then(|z| z.issues.get_mut(1))
                    {
                        issue.pull_requests = vec![link];
                    }
                });
                self.show_zapper_page(ZapperPage::PullRequest(url), cx);
            }
            "threads" => {
                self.zapper_ui.finished_open = true;
                self.show_zapper_page(ZapperPage::Section(ZapperTab::Threads), cx);
            }
            "history-menu" => self.zapper_ui.overflow.open(()),
            "history" => self.show_zapper_page(ZapperPage::History, cx),
            "close" => {
                let active = self.zapper_ui.tabs.active().clone();
                self.close_zapper_page(&active, cx);
            }
            _ => self.show_zapper_page(ZapperPage::Section(ZapperTab::Overview), cx),
        }
        // Page changes ask for a reload; the seeded files stand in for it.
        self.zapper_ui.loaded =
            Some((zapper.id.clone(), zapper.revision, zapper.activity_revision));
        cx.notify();
    }
}
