//! Coordinator chrome; commands use the same engine authority as external clients.
use super::zot_forms::{Form, Kind, ZotAfter};
use super::*;
use zuse_protocol::presentation::zot_mascots;
use zuse_protocol::presentation::zot_panel::{ZotTab, ZotTabs, zot_owns_chat};
use zuse_protocol::zots::*;

#[derive(Default)]
pub(super) struct ZotUi {
    /// New Zot or Zot settings. Outlives the route so Back returns to the draft.
    pub(super) page: Option<Form>,
    pub(super) dialog: Option<Form>,
    pub(super) panel_hidden: bool,
    tabs: ZotTabs,
    pub(super) issues: Option<Entity<zuse_viewport::zot_issues::IssuePanel>>,
    pub(super) pull_request: Option<String>,
    pull_request_page: Option<(
        String,
        Entity<crate::pull_request_detail::PullRequestDetailPage>,
    )>,
    pull_request_subscription: Option<Subscription>,
    files: Vec<ZotFile>,
    /// The Zot `files` came from, kept while a reload is in flight.
    files_zot: Option<String>,
    /// Threads just archived here. The engine applies it on its next pass, a
    /// few seconds later; until then the row should already be gone.
    archiving: std::collections::HashMap<String, std::time::Instant>,
    /// Right-click menu of a sidebar Zapper row: its id and where it opened.
    pub(super) menu: popover::Popup<(String, Point<Pixels>)>,
    /// The Zapper waiting on "Delete Zapper?" confirmation.
    pub(super) delete_confirm: Option<String>,
    pub(super) loaded: Option<(String, u32, u32)>,
    events: Vec<ZotEvent>,
    loading: bool,
    pub(super) pending: bool,
    pub(super) retry: Option<ZotCommand>,
    /// Set while the issue panel hands a command to `send_zot_command`, so
    /// only its own results close or fail its editor.
    pub(super) issue_command: bool,
    history: bool,
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
    div()
        .id(SharedString::from(id))
        .role(gpui::Role::Button)
        .aria_label(label)
        .size(px(24.0))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(6.0))
        .bg(gpui::rgb(0x242424))
        .cursor_pointer()
        .text_color(theme.text_muted)
        .hover(|d| d.text_color(theme.text_strong))
        .tip(label)
        .child(icon(glyph).size(px(13.0)))
        .on_click(cx.listener(move |this, event, window, cx| {
            cx.stop_propagation();
            callback(this, event, window, cx)
        }))
        .into_any_element()
}

/// One pixel character, `pixel` logical pixels per sprite cell.
pub(super) fn zot_mascot(avatar: &ZotAvatar, pixel: f32) -> impl IntoElement + use<> {
    let runs = zot_mascots::sprite_runs(avatar);
    let side = pixel * zot_mascots::SPRITE_SIZE as f32;
    gpui::canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            for &(x, y, width, rgb) in &runs {
                let cell = gpui::point(px(f32::from(x) * pixel), px(f32::from(y) * pixel));
                let extent = gpui::size(px(f32::from(width) * pixel), px(pixel));
                window.paint_quad(gpui::fill(
                    gpui::Bounds::new(bounds.origin + cell, extent),
                    gpui::rgb(rgb),
                ));
            }
        },
    )
    .size(px(side))
    .flex_none()
}

pub(super) fn zot_avatar(zot: &Zot) -> ZotAvatar {
    zot.config
        .avatar
        .clone()
        .unwrap_or_else(|| zot_mascots::default_avatar(&zot.id))
}

pub(super) const ZOT_PANEL_WIDTH: f32 = 520.0;

/// A document worth listing: seeded placeholders stay empty, and the
/// instructions file mirrors the purpose edited with the Zot itself.
fn is_document(file: &ZotFile) -> bool {
    file.path != "INSTRUCTIONS.md" && !file.content.trim().is_empty()
}

/// Drive and History only appear once they have something to show. `keep`
/// holds the open tab while its files are still loading.
fn visible_tabs(files: &[ZotFile], keep: Option<ZotTab>) -> Vec<ZotTab> {
    ZotTab::ALL
        .into_iter()
        .filter(|tab| {
            Some(*tab) == keep
                || match tab {
                    ZotTab::Drive => files.iter().any(is_document),
                    ZotTab::History => files.iter().any(|f| f.version > 1),
                    _ => true,
                }
        })
        .collect()
}

/// Where one Zot conversation stands in the sidebar.
#[derive(Clone, Copy)]
enum ThreadPhase {
    Working,
    NeedsYou(zuse_protocol::ChatIndicator),
    /// No message has run yet, so it has no worktree or transcript.
    NotStarted,
    Done,
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

/// The row's leading mark, in the same vocabulary as project rows.
fn thread_mark(
    key: String,
    phase: ThreadPhase,
    awaiting: Option<zuse_protocol::AwaitingInputKind>,
    theme: &Theme,
) -> AnyElement {
    let glyph = |path: &'static str, tone: gpui::Hsla| {
        icon(path)
            .size(px(13.0))
            .flex_none()
            .text_color(tone)
            .into_any_element()
    };
    match phase {
        ThreadPhase::Working => crate::loaders::retro_loader(key, 13.0, theme.loader_glyph()),
        ThreadPhase::NeedsYou(status) => {
            match super::spaces::status_glyph(status, false, false, awaiting, theme) {
                Some((path, tone)) => glyph(path, tone),
                None => div().size(px(13.0)).flex_none().into_any_element(),
            }
        }
        ThreadPhase::NotStarted => glyph(icons::STATUS_QUEUED, theme.text_faint),
        ThreadPhase::Done => glyph(icons::GIT_BRANCH, theme.text_faint),
    }
}

impl Shell {
    pub(super) fn selected_zot(&self, cx: &App) -> Option<Zot> {
        let state = self.state.read(cx);
        state
            .zots
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
    pub(super) fn render_zot_sidebar(
        &mut self,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let state = self.state.read(cx);
        if !state
            .engine()
            .is_some_and(|e| e.engine_info().supports(zuse_protocol::zots::CAPABILITY))
        {
            return Empty.into_any_element();
        }
        let zots = state.zots.clone();
        // A Zot page owns the main area, so no conversation reads as current.
        let creating = self.creating_zot();
        let selected = state.selected_chat.clone().filter(|_| !creating);
        let titles: std::collections::HashMap<String, String> = state
            .chats
            .iter()
            .filter_map(|c| Some((c.id.clone(), c.title.clone()?)))
            .collect();
        let header = div()
            .flex()
            .items_center()
            .justify_between()
            .h(px(26.0))
            .px(px(8.0))
            .text_size(crate::typography::ui_rems(11.0))
            .text_color(theme.text_faint)
            .child("Zappers")
            .child(
                div()
                    .id("new-zot")
                    .role(gpui::Role::Button)
                    .aria_label("New Zapper")
                    .size(px(20.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(5.0))
                    .cursor_pointer()
                    .when(creating, |d| d.bg(theme.bg_selected))
                    .hover(|d| d.bg(theme.element_hover))
                    .tip("New Zapper")
                    .child(
                        icon(icons::PLUS)
                            .size(px(13.0))
                            .text_color(theme.text_muted),
                    )
                    .on_click(cx.listener(|this, _, _, cx| this.open_new_zot(cx))),
            );
        let mut list = div()
            .flex()
            .flex_col()
            .px(px(8.0))
            .pt(px(6.0))
            .pb(px(8.0))
            .gap(px(1.0))
            .child(header);
        for zot in zots {
            let id = zot.coordinator_chat_id.clone();
            let active = selected.as_deref() == Some(id.as_str())
                || zot
                    .previous_coordinator_chats
                    .iter()
                    .any(|c| Some(c) == selected.as_ref());
            let needs = zot.decisions.iter().filter(|d| d.answer.is_none()).count();
            let (coordinator, _) = self.zot_thread_phase(&id, cx);
            // Finished threads leave the sidebar and stay under the Threads tab.
            let threads: Vec<_> = zot
                .workers
                .iter()
                .filter(|worker| !self.zot_thread_archived(&worker.chat_id, cx))
                .map(|worker| {
                    let (phase, awaiting) = self.zot_thread_phase(&worker.chat_id, cx);
                    (worker, phase, awaiting)
                })
                .filter(|(worker, phase, _)| {
                    !matches!(phase, ThreadPhase::Done)
                        || selected.as_deref() == Some(worker.chat_id.as_str())
                })
                .collect();
            let working = matches!(coordinator, ThreadPhase::Working)
                || threads
                    .iter()
                    .any(|(_, phase, _)| matches!(phase, ThreadPhase::Working));
            let waiting = if needs == 1 {
                "1 decision waiting for you".to_owned()
            } else {
                format!("{needs} decisions waiting for you")
            };
            // Stays lit while its right-click menu is open.
            let menu_open = self
                .zot_ui
                .menu
                .get()
                .is_some_and(|(menu_zot, _)| menu_zot == &zot.id);
            list = list.child(
                div()
                    .id(SharedString::from(format!("zot-{}", zot.id)))
                    .role(gpui::Role::Button)
                    .aria_label(SharedString::from(zot.config.name.clone()))
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
                    .child(zot_mascot(&zot_avatar(&zot), 1.0))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .child(zot.config.name.clone()),
                    )
                    .when(zot.status != ZotStatus::Active, |d| {
                        d.child(
                            div()
                                .text_size(crate::typography::ui_rems(11.0))
                                .text_color(theme.text_faint)
                                .child(format!("{:?}", zot.status)),
                        )
                    })
                    .when(working, |d| {
                        d.child(crate::loaders::retro_loader(
                            format!("zot-working-{}", zot.id),
                            13.0,
                            theme.loader_glyph(),
                        ))
                    })
                    .when(!working && needs > 0, |d| {
                        d.child(
                            div()
                                .id(SharedString::from(format!("zot-decisions-{}", zot.id)))
                                .aria_label(SharedString::from(waiting.clone()))
                                .tip(waiting.clone())
                                .child(
                                    icon(icons::STATUS_QUESTION)
                                        .size(px(13.0))
                                        .text_color(theme.warning),
                                ),
                        )
                    })
                    .on_click(cx.listener(move |this, _, _, cx| this.open_chat(id.clone(), cx)))
                    .on_mouse_down(MouseButton::Right, {
                        let zot_id = zot.id.clone();
                        cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                            this.zot_ui.menu.open((zot_id.clone(), event.position));
                            cx.stop_propagation();
                            cx.notify();
                        })
                    }),
            );
            // A Zot's threads live under it instead of under their projects.
            let count = threads.len();
            for (index, (worker, phase, awaiting)) in threads.into_iter().enumerate() {
                let chat = worker.chat_id.clone();
                let current = selected.as_deref() == Some(chat.as_str());
                let title = titles
                    .get(&chat)
                    .cloned()
                    .unwrap_or_else(|| worker.objective.clone());
                let row_group = SharedString::from(format!("zot-sidebar-thread-{chat}"));
                let archive = {
                    let zot = zot.clone();
                    let chat = chat.clone();
                    thread_action(
                        format!("zot-sidebar-archive-{chat}"),
                        icons::ARCHIVE_MINIMALISTIC,
                        "Archive",
                        theme,
                        move |this, _, _, cx| this.archive_zot_thread(&zot, chat.clone(), cx),
                        cx,
                    )
                };
                list = list.child(
                    div()
                        .id(SharedString::from(format!("zot-thread-{chat}")))
                        .group(row_group.clone())
                        .role(gpui::Role::Button)
                        .aria_label(SharedString::from(title.clone()))
                        .h(px(26.0))
                        .ml(px(14.0))
                        .pl(px(10.0))
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
                        // Tree guide: a vertical rule joining the threads to their Zot.
                        .child(
                            div()
                                .absolute()
                                .left(px(0.0))
                                .top(px(0.0))
                                .w(px(1.0))
                                .h(px(if index + 1 == count { 13.0 } else { 26.0 }))
                                .bg(theme.border),
                        )
                        .child(
                            div()
                                .absolute()
                                .left(px(0.0))
                                .top(px(13.0))
                                .w(px(6.0))
                                .h(px(1.0))
                                .bg(theme.border),
                        )
                        .child(thread_mark(
                            format!("zot-thread-working-{chat}"),
                            phase,
                            awaiting,
                            theme,
                        ))
                        .child(div().flex_1().min_w_0().truncate().child(title))
                        .when(matches!(phase, ThreadPhase::NotStarted), |d| {
                            d.tip("Hasn't started yet")
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
        list.into_any_element()
    }
    /// Archived threads leave every Zapper list and live under Archived.
    fn zot_thread_archived(&self, chat: &str, cx: &App) -> bool {
        self.state.read(cx).chat_is_archived(chat)
            || self
                .zot_ui
                .archiving
                .get(chat)
                .is_some_and(|at| at.elapsed() < std::time::Duration::from_secs(10))
    }

    /// The Zapper a thread works for, if any.
    pub(super) fn zot_of_thread(&self, chat_id: &str, cx: &App) -> Option<Zot> {
        self.state
            .read(cx)
            .zots
            .iter()
            .find(|z| z.workers.iter().any(|w| w.chat_id == chat_id))
            .cloned()
    }

    fn close_zot_menu(&mut self, cx: &mut Context<Self>) {
        if self.zot_ui.menu.begin_close() {
            popover::reap_popup(cx, |shell: &mut Self| &mut shell.zot_ui.menu);
            cx.notify();
        }
    }

    /// Escape closes the Zapper menu, then its delete confirmation.
    pub(super) fn dismiss_zot_overlay(&mut self, cx: &mut Context<Self>) -> bool {
        if self.zot_ui.menu.is_open() {
            self.close_zot_menu(cx);
            return true;
        }
        if self.zot_ui.delete_confirm.take().is_some() {
            cx.notify();
            return true;
        }
        false
    }

    pub(super) fn zot_overlay_open(&self) -> bool {
        self.zot_ui.menu.get().is_some() || self.zot_ui.delete_confirm.is_some()
    }

    /// The settings page of `zot` closes when the Zapper itself goes away.
    fn after_removing(&self, zot: &Zot) -> ZotAfter {
        if self
            .zot_ui
            .page
            .as_ref()
            .is_some_and(|page| page.id == zot.id)
        {
            ZotAfter::LeavePage
        } else {
            ZotAfter::Stay
        }
    }

    pub(super) fn render_zot_overlays(
        &mut self,
        viewport: gpui::Size<Pixels>,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let theme = Theme::of(cx).for_popup();
        let zots = self.state.read(cx).zots.clone();
        let mut overlays = Vec::new();
        if let Some((zot_id, position)) = self.zot_ui.menu.get().cloned() {
            match zots.iter().find(|z| z.id == zot_id).cloned() {
                Some(zot) => {
                    let row = |key: &str, glyph: &'static str, label: &str, danger: bool| {
                        let tone = if danger {
                            theme.danger
                        } else {
                            theme.text_muted
                        };
                        popover::menu_row(&theme, false, format!("zot-menu-{key}-{zot_id}"))
                            .id(SharedString::from(format!("zot-menu-{key}")))
                            .when(danger, |d| d.text_color(theme.danger))
                            .child(icon(glyph).size(px(16.0)).text_color(tone))
                            .child(SharedString::from(label.to_owned()))
                    };
                    let z = zot.clone();
                    let new_thread = row("thread", icons::PLUS, "New thread", false).on_click(
                        cx.listener(move |this, _, _, cx| {
                            this.close_zot_menu(cx);
                            this.open_zot_dialog(z.clone(), Kind::Thread(None), cx);
                        }),
                    );
                    let z = zot.clone();
                    let settings = row("settings", icons::SETTINGS, "Settings", false).on_click(
                        cx.listener(move |this, _, _, cx| {
                            this.close_zot_menu(cx);
                            this.open_zot_editor(z.clone(), cx);
                        }),
                    );
                    let z = zot.clone();
                    let archive = row(
                        "archive",
                        icons::ARCHIVE_MINIMALISTIC,
                        "Archive with threads",
                        false,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.close_zot_menu(cx);
                        let after = this.after_removing(&z);
                        this.send_zot_command(
                            z.id.clone(),
                            Some(z.revision),
                            ZotAction::SetStatus {
                                status: ZotStatus::Archived,
                            },
                            after,
                            cx,
                        );
                    }));
                    let z = zot.clone();
                    let delete = row("delete", icons::TRASH_BIN_MINIMALISTIC, "Delete…", true)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.close_zot_menu(cx);
                            this.zot_ui.delete_confirm = Some(z.id.clone());
                            cx.notify();
                        }));
                    let menu = popover::popover_card(&theme)
                        .w(px(216.0))
                        .flex()
                        .flex_col()
                        .on_mouse_down_out(cx.listener(|this, _, _, cx| this.close_zot_menu(cx)))
                        .child(new_thread)
                        .child(settings)
                        .when(zot.status != ZotStatus::Archived, |menu| {
                            menu.child(archive)
                        })
                        .child(popover::menu_separator())
                        .child(delete)
                        .into_any_element();
                    overlays.push(popover::menu_at(
                        "zot-context-menu",
                        position,
                        menu,
                        self.zot_ui.menu.closing_since(),
                    ));
                }
                // The Zapper vanished under the open menu.
                None => self.close_zot_menu(cx),
            }
        }
        if let Some(zot_id) = self.zot_ui.delete_confirm.clone() {
            let Some(zot) = zots.into_iter().find(|z| z.id == zot_id) else {
                self.zot_ui.delete_confirm = None;
                return overlays;
            };
            let name = crate::transcript::single_line(&zot.config.name);
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
                                "delete-zot-cancel",
                            )
                            .id("delete-zot-cancel")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.zot_ui.delete_confirm = None;
                                cx.notify();
                            })),
                        )
                        .child(
                            popover::btn_danger(&theme, "Delete")
                                .id("delete-zot-confirm")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.zot_ui.delete_confirm = None;
                                    let after = this.after_removing(&zot);
                                    this.send_zot_command(
                                        zot.id.clone(),
                                        Some(zot.revision),
                                        ZotAction::Delete,
                                        after,
                                        cx,
                                    );
                                })),
                        ),
                )
                .into_any_element();
            overlays.push(popover::modal("delete-zot-dialog", viewport, card));
        }
        overlays
    }

    pub(super) fn archive_zot_thread(
        &mut self,
        zot: &Zot,
        chat_id: String,
        cx: &mut Context<Self>,
    ) {
        if self.zot_ui.pending {
            return;
        }
        let viewing = self
            .state
            .read(cx)
            .selected_chat_row()
            .is_some_and(|chat| chat.id == chat_id);
        // Don't leave the user on a thread that just left the list.
        if viewing {
            self.open_chat(zot.coordinator_chat_id.clone(), cx);
        }
        self.zot_ui
            .archiving
            .insert(chat_id.clone(), std::time::Instant::now());
        self.send_zot_command(
            zot.id.clone(),
            Some(zot.revision),
            ZotAction::ArchiveWorker { chat_id },
            ZotAfter::Stay,
            cx,
        );
        cx.notify();
    }

    fn zot_thread_phase(
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
    fn zot_thread_status(&self, chat: &str, cx: &App) -> Option<zuse_protocol::SessionStatus> {
        self.state
            .read(cx)
            .sessions
            .iter()
            .find(|s| s.chat_id == chat && (chrono::Utc::now() - s.updated_at).num_seconds() < 45)
            .map(|s| s.status.clone())
    }
    fn zot_status_color(&self, chat: &str, theme: &Theme, cx: &App) -> gpui::Hsla {
        use zuse_protocol::SessionStatus as S;
        match self.zot_thread_status(chat, cx) {
            Some(S::Working) => theme.busy,
            Some(S::AwaitingInput) => theme.warning,
            Some(S::Errored) => theme.danger,
            _ => theme.text_faint.opacity(0.5),
        }
    }
    fn zot_tab_bar(
        &self,
        visible: &[ZotTab],
        editing: bool,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let tabs = self.zot_ui.tabs;
        let detail_open = self.zot_ui.pull_request.is_some()
            || self
                .zot_ui
                .issues
                .as_ref()
                .is_some_and(|p| p.read(cx).selected_code().is_some());
        let selected_bg: gpui::Hsla = gpui::rgb(0x242424).into();
        let mut strip = div().flex().items_center().gap(px(2.0));
        for &tab in visible {
            let active = tab == tabs.active && !editing && !detail_open;
            let path = match tab {
                ZotTab::Overview => icons::LIST,
                ZotTab::Issues => icons::CLOCK_CIRCLE,
                ZotTab::Threads => icons::CHAT_ROUND_LINE,
                ZotTab::Drive => icons::FOLDER_OPEN,
                ZotTab::History => icons::HISTORY,
            };
            let label = tab.label();
            let color = if active {
                theme.text_strong
            } else {
                theme.text_muted
            };
            let base = div()
                .id(SharedString::from(format!("zot-tab-{label}")))
                .role(gpui::Role::Tab)
                .aria_label(label)
                .h(px(26.0))
                .px(px(7.0))
                .flex()
                .items_center()
                .rounded(px(7.0))
                .cursor_pointer()
                .text_size(crate::typography::ui_rems(12.5))
                .text_color(color)
                .when(!active, |d| {
                    d.hover(|d| d.bg(theme.element_hover).text_color(theme.text))
                        .tip(label)
                })
                .child(if tab == ZotTab::Issues {
                    zuse_viewport::zot_issues::status_icon(ZotIssueStatus::Todo).into_any_element()
                } else {
                    icon(path)
                        .size(px(12.))
                        .text_color(color)
                        .into_any_element()
                })
                .on_click(cx.listener(move |this, _, _, cx| {
                    if this.zot_editor_open(cx) {
                        this.zot_ui.page = None;
                        cx.notify();
                    }
                    this.zot_ui.pull_request = None;
                    if let Some(panel) = &this.zot_ui.issues {
                        panel.update(cx, |p, cx| p.show_list(cx));
                    }
                    if this.zot_ui.tabs.select(tab) {
                        let history = tab == ZotTab::History;
                        if this.zot_ui.history != history {
                            this.zot_ui.history = history;
                            this.zot_ui.loaded = None;
                        }
                        cx.notify();
                    }
                }));
            strip = strip.child(
                base.when(
                    matches!(tab, ZotTab::Overview | ZotTab::Issues | ZotTab::Drive) || active,
                    |d| d.child(div().ml(px(6.)).child(label)),
                )
                .when(active, |d| d.bg(selected_bg)),
            );
        }
        let extra = self
            .zot_ui
            .pull_request
            .as_ref()
            .map(|u| format!("#{}", u.rsplit('/').next().unwrap_or("")))
            .or_else(|| {
                self.zot_ui
                    .issues
                    .as_ref()
                    .and_then(|p| p.read(cx).selected_code())
            });
        if let Some(code) =
            extra.filter(|_| !editing && matches!(tabs.active, ZotTab::Issues | ZotTab::Overview))
        {
            strip = strip.child(
                div()
                    .flex()
                    .items_center()
                    .px(px(8.))
                    .h(px(26.))
                    .gap(px(6.))
                    .rounded(px(7.))
                    .bg(gpui::rgb(0x242424))
                    .text_color(theme.text)
                    .child(if self.zot_ui.pull_request.is_some() {
                        icon(icons::GIT_BRANCH)
                            .size(px(12.))
                            .text_color(theme.warning)
                            .into_any_element()
                    } else {
                        zuse_viewport::zot_issues::status_icon(
                            self.zot_ui
                                .issues
                                .as_ref()
                                .and_then(|p| p.read(cx).selected_status())
                                .unwrap_or(ZotIssueStatus::Todo),
                        )
                        .into_any_element()
                    })
                    .child(code),
            );
        }
        let icon_button = |id: &'static str, path: &'static str, tip: &'static str| {
            div()
                .id(id)
                .role(gpui::Role::Button)
                .aria_label(tip)
                .size(px(28.0))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(7.0))
                .cursor_pointer()
                .hover(|d| d.bg(theme.element_hover))
                .tip(tip)
                .child(icon(path).size(px(12.0)).text_color(theme.text_muted))
        };
        let zot = self.selected_zot(cx);
        let new_doc = zot.clone();
        div()
            .flex()
            .items_center()
            .h(px(40.0))
            .flex_none()
            .px(px(10.0))
            .border_b_1()
            .border_color(theme.border)
            .child(strip)
            .child(div().flex_1())
            .when(tabs.active == ZotTab::Drive, |d| {
                d.child(
                    icon_button("zot-new-document", icons::DOCUMENT_ADD, "New document").on_click(
                        cx.listener(move |this, _, _, cx| {
                            let file = ZotFile {
                                path: String::new(),
                                version: 0,
                                content: String::new(),
                            };
                            if let Some(zot) = new_doc.clone() {
                                this.open_zot_dialog(zot, Kind::File(file), cx)
                            }
                        }),
                    ),
                )
            })
            .child(
                icon_button("zot-edit", icons::PEN, "Edit Zapper")
                    .when(editing, |d| d.bg(theme.element_active))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if this.zot_editor_open(cx) {
                            this.zot_ui.page = None;
                            cx.notify();
                        } else if let Some(zot) = zot.clone() {
                            this.open_zot_editor(zot, cx);
                        }
                    })),
            )
            .into_any_element()
    }

    pub(super) fn render_zot_inspector(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(zot) = self.selected_zot(cx) else {
            return Empty.into_any_element();
        };
        let theme = Theme::of(cx).clone();
        let key = (zot.id.clone(), zot.revision, zot.activity_revision);
        if self.zot_ui.loaded.as_ref() != Some(&key) && !self.zot_ui.loading {
            if let Some(engine) = self.state.read(cx).engine().cloned() {
                self.zot_ui.loading = true;
                let requested_history = self.zot_ui.history;
                let requested_zot = zot.id.clone();
                let request = serde_json::json!({"zotId":zot.id,"targetDeviceId":zot.config.home_device_id,"history":self.zot_ui.history});
                cx.spawn(async move |this, cx| {
                    let result = engine
                        .client()
                        .call(zuse_protocol::control::ZOT_FILES, request.clone())
                        .await;
                    let events = engine
                        .client()
                        .call(zuse_protocol::control::ZOT_EVENTS, request)
                        .await;
                    let _ = this.update(cx, |this, cx| {
                        this.zot_ui.loading = false;
                        if let Ok(value) = events {
                            if let Ok(events) = serde_json::from_value(value) {
                                this.zot_ui.events = events;
                            }
                        }
                        // A tab switch mid-flight wants the other file set;
                        // leaving `loaded` unset makes the next render fetch it.
                        if this.zot_ui.history == requested_history
                            && let Ok(value) = result
                            && let Ok(files) = serde_json::from_value(value)
                        {
                            this.zot_ui.files = files;
                            this.zot_ui.files_zot = Some(requested_zot);
                            this.zot_ui.loaded = Some(key);
                        }
                        cx.notify();
                    });
                })
                .detach();
            }
        }
        if self
            .zot_ui
            .issues
            .as_ref()
            .is_some_and(|p| p.read(cx).zot_id() != zot.id)
        {
            self.zot_ui.pull_request = None;
        }
        // A reload must not read as "no files": that used to hide the tab just
        // clicked and bounce the panel to Overview.
        let files = if self.zot_ui.files_zot.as_deref() == Some(zot.id.as_str()) {
            self.zot_ui.files.clone()
        } else {
            vec![]
        };
        let settled = self
            .zot_ui
            .loaded
            .as_ref()
            .is_some_and(|(id, _, _)| id == &zot.id);
        let active = self.zot_ui.tabs.active;
        let visible = visible_tabs(&files, (!settled).then_some(active));
        if !visible.contains(&active) {
            self.zot_ui.tabs.select(ZotTab::Overview);
        }
        let editing = self.zot_editor_open(cx);
        let tab_bar = self.zot_tab_bar(&visible, editing, &theme, cx);
        if editing {
            let editor = self.render_zot_editor(&zot, window, cx);
            return Self::zot_inspector_frame(&theme, tab_bar, editor);
        }
        if let Some(url) = self.zot_ui.pull_request.clone() {
            let page = self
                .zot_ui
                .pull_request_page
                .as_ref()
                .filter(|(loaded, _)| loaded == &url)
                .map(|(_, page)| page.clone());
            if let Some(page) = page {
                let mut content = div().flex_1().min_h_0().flex().flex_col();
                if let Some(issue) = zot
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
                                this.zot_ui.pull_request = None;
                                if let Some(panel) = &this.zot_ui.issues {
                                    panel.update(cx, |p, cx| p.show_issue(id.clone(), cx));
                                }
                                cx.notify();
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
                return Self::zot_inspector_frame(
                    &theme,
                    tab_bar,
                    content.child(page).into_any_element(),
                );
            }
            let device = zot.config.home_device_id.clone();
            // Page construction starts RPC reads. Defer it until this render
            // finishes, and keep it inside the Zot inspector's own navigation.
            cx.defer_in(window, move |this, window, cx| {
                if this.zot_ui.pull_request.as_ref() != Some(&url)
                    || this
                        .zot_ui
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
                this.zot_ui.pull_request_subscription =
                    Some(cx.observe(&page, |_, _, cx| cx.notify()));
                this.zot_ui.pull_request_page = Some((url, page));
                cx.notify();
            });
            return Self::zot_inspector_frame(
                &theme,
                tab_bar,
                div()
                    .p(px(16.))
                    .text_color(theme.text_muted)
                    .child("Loading pull request…")
                    .into_any_element(),
            );
        }
        let body = match self.zot_ui.tabs.active {
            ZotTab::Overview => self.zot_issue_surface(&zot, true, cx),
            ZotTab::Issues => self.zot_issue_surface(&zot, false, cx),
            ZotTab::Threads => {
                let threads = self.zot_threads(&zot, &theme, cx);
                let overview = self.zot_overview(&zot, &theme, cx);
                div()
                    .flex()
                    .flex_col()
                    .gap(px(18.))
                    .child(threads)
                    .child(overview)
                    .into_any_element()
            }
            ZotTab::Drive => self.zot_drive(&zot, files, &theme, cx),
            ZotTab::History => self.zot_history(&zot, files, &theme, cx),
        };
        let body = div()
            .id("zot-inspector-body")
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
        Self::zot_inspector_frame(&theme, tab_bar, body)
    }

    fn zot_inspector_frame(theme: &Theme, tab_bar: AnyElement, body: AnyElement) -> AnyElement {
        div()
            .id("zot-inspector")
            .bg(zuse_viewport::palette::rgb(0x101010))
            .w(px(ZOT_PANEL_WIDTH))
            .h_full()
            .flex_none()
            .flex()
            .flex_col()
            // The content row spans the window; clear the title bar's controls.
            .pt(px(Theme::TITLEBAR_HEIGHT))
            .border_l_1()
            .border_color(theme.border)
            .child(tab_bar)
            .child(body)
            .into_any_element()
    }

    fn zot_issue_surface(
        &mut self,
        zot: &Zot,
        overview: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        use zuse_viewport::zot_issues::{IssueEvent, IssuePanel};
        if self.zot_ui.issues.is_none() {
            let panel = cx.new(|_| IssuePanel::new(zot.clone(), overview));
            cx.subscribe(&panel, |this, _, event, cx| match event {
                IssueEvent::Command(action, revision) => {
                    if let Some(zot) = this.selected_zot(cx) {
                        let mut action = action.clone();
                        if let ZotAction::Delegate { model, .. } = &mut action {
                            *model = this.zot_thread_model(&zot, cx);
                        }
                        this.zot_ui.issue_command = true;
                        this.send_zot_command(zot.id, Some(*revision), action, ZotAfter::Stay, cx);
                    }
                }
                IssueEvent::OpenThread(id) => this.open_chat(id.clone(), cx),
                IssueEvent::OpenDocument(file) => {
                    if let Some(zot) = this.selected_zot(cx) {
                        this.open_zot_dialog(zot, Kind::File(file.clone()), cx);
                    }
                }
                IssueEvent::OpenPullRequest(url) => {
                    this.zot_ui.pull_request = Some(url.clone());
                    cx.notify();
                }
            })
            .detach();
            cx.observe(&panel, |_, _, cx| cx.notify()).detach();
            self.zot_ui.issues = Some(panel);
        }
        let panel = self.zot_ui.issues.as_ref().unwrap().clone();
        panel.update(cx, |p, cx| {
            p.sync(zot.clone(), overview, self.zot_ui.pending, cx);
            p.files(self.zot_ui.files.clone(), cx);
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
    fn zot_overview(&mut self, zot: &Zot, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        use zuse_protocol::SessionStatus as S;
        let working: Vec<_> = zot
            .workers
            .iter()
            .filter(|w| self.zot_thread_status(&w.chat_id, cx) == Some(S::Working))
            .cloned()
            .collect();
        let (status_color, status) = match zot.status {
            ZotStatus::Active => (theme.success, "Active"),
            ZotStatus::Paused => (theme.warning, "Paused"),
            ZotStatus::Stopped => (theme.text_faint, "Stopped"),
            ZotStatus::Archived => (theme.text_faint, "Archived"),
        };
        let toggle = if zot.status == ZotStatus::Active {
            ZotStatus::Paused
        } else {
            ZotStatus::Active
        };
        let (id, rev) = (zot.id.clone(), zot.revision);
        let hero = div()
            .flex()
            .items_center()
            .gap(px(12.0))
            .child(
                div()
                    .size(px(60.0))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(12.0))
                    .bg(theme.element_hover)
                    .child(zot_mascot(&zot_avatar(zot), 3.0)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .child(
                        div()
                            .truncate()
                            .text_size(crate::typography::ui_rems(16.0))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(theme.text_strong)
                            .child(zot.config.name.clone()),
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
                matches!(zot.status, ZotStatus::Active | ZotStatus::Paused),
                |d| {
                    d.child(action(
                        "zot-pause",
                        if toggle == ZotStatus::Paused {
                            "Pause"
                        } else {
                            "Resume"
                        },
                        theme,
                        move |this, _, _, cx| {
                            this.send_zot_command(
                                id.clone(),
                                Some(rev),
                                ZotAction::SetStatus { status: toggle },
                                ZotAfter::Stay,
                                cx,
                            )
                        },
                        cx,
                    ))
                },
            );
        let decisions: Vec<_> = zot
            .decisions
            .iter()
            .filter(|d| d.answer.is_none())
            .cloned()
            .collect();
        let events: Vec<_> = self
            .zot_ui
            .events
            .iter()
            .filter(|e| e.zot_id == zot.id && !e.acknowledged && e.kind != "settled")
            .cloned()
            .collect();
        let waiting = decisions.len() + events.len();
        let stat = |value: usize, label: &'static str, alert: bool| {
            div()
                .flex_1()
                .py(px(10.0))
                .flex()
                .flex_col()
                .items_center()
                .gap(px(2.0))
                .rounded(px(10.0))
                .bg(crate::settings::widgets::block_fill(theme))
                .child(
                    div()
                        .text_size(crate::typography::ui_rems(18.0))
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .text_color(if alert {
                            theme.warning
                        } else {
                            theme.text_strong
                        })
                        .child(value.to_string()),
                )
                .child(
                    div()
                        .text_size(crate::typography::ui_rems(11.0))
                        .text_color(theme.text_faint)
                        .child(label),
                )
        };
        let mut view = div()
            .flex()
            .flex_col()
            .gap(px(16.0))
            .child(hero)
            .when(!zot.config.purpose.trim().is_empty(), |d| {
                d.child(
                    div()
                        .text_size(crate::typography::ui_rems(12.5))
                        .line_height(crate::typography::ui_rems(18.0))
                        .text_color(theme.text_muted)
                        .child(zot.config.purpose.clone()),
                )
            })
            .child(
                div()
                    .flex()
                    .gap(px(6.0))
                    .child(stat(
                        zot.workers
                            .iter()
                            .filter(|w| !self.zot_thread_archived(&w.chat_id, cx))
                            .count(),
                        "Threads",
                        false,
                    ))
                    .child(stat(working.len(), "Working", false))
                    .child(stat(waiting, "Needs you", waiting > 0)),
            );
        if waiting > 0 {
            let mut needs = div()
                .flex()
                .flex_col()
                .gap(px(2.0))
                .child(section("Needs you", theme));
            for decision in decisions {
                let z = zot.clone();
                needs = needs.child(list_row(
                    format!("zot-decision-{}", decision.id),
                    icons::INFO_CIRCLE,
                    theme.warning,
                    decision.question.clone(),
                    Some("Answer".into()),
                    theme,
                    move |this, _, _, cx| {
                        this.open_zot_dialog(z.clone(), Kind::Decision(decision.clone()), cx);
                    },
                    cx,
                ));
            }
            for event in events {
                let id = event.worker_chat_id.clone();
                needs = needs.child(list_row(
                    format!("zot-event-{}", event.id),
                    icons::CHAT_ROUND_LINE,
                    theme.text_muted,
                    event.summary,
                    None,
                    theme,
                    move |this, _, _, cx| this.open_chat(id.clone(), cx),
                    cx,
                ));
            }
            view = view.child(needs);
        }
        if !working.is_empty() {
            let mut running = div()
                .flex()
                .flex_col()
                .gap(px(2.0))
                .child(section("Working now", theme));
            for worker in working {
                let id = worker.chat_id.clone();
                running = running.child(list_row(
                    format!("zot-working-{id}"),
                    icons::CHAT_ROUND_LINE,
                    theme.busy,
                    worker.objective.clone(),
                    None,
                    theme,
                    move |this, _, _, cx| this.open_chat(id.clone(), cx),
                    cx,
                ));
            }
            view = view.child(running);
        }
        let names: Vec<(String, String)> = {
            let state = self.state.read(cx);
            zot.config
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
        let mut repositories = div()
            .flex()
            .flex_col()
            .gap(px(2.0))
            .child(section("Repositories", theme));
        if names.is_empty() {
            let z = zot.clone();
            repositories = repositories.child(list_row(
                "zot-add-repositories",
                icons::PLUS,
                theme.text_muted,
                "Choose repositories".to_owned(),
                None,
                theme,
                move |this, _, _, cx| this.open_zot_editor(z.clone(), cx),
                cx,
            ));
        }
        for (id, name) in names {
            repositories = repositories.child(
                div()
                    .id(SharedString::from(format!("zot-overview-repo-{id}")))
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .py(px(5.0))
                    .text_size(crate::typography::ui_rems(12.5))
                    .text_color(theme.text)
                    .child(
                        icon(icons::FOLDER)
                            .size(px(13.0))
                            .flex_none()
                            .text_color(theme.text_muted),
                    )
                    .child(div().flex_1().min_w_0().truncate().child(name)),
            );
        }
        view.child(repositories).into_any_element()
    }

    pub(super) fn zot_manage(
        &mut self,
        zot: &Zot,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut manage = div()
            .flex()
            .flex_col()
            .gap(px(2.0))
            .child(section("Manage", theme));
        for (key, glyph, label, kind) in [
            ("zot-fresh", icons::RESTART, "Start a fresh conversation", 0),
            ("zot-stop", icons::STOP, "Stop Zapper", 1),
            (
                "zot-archive",
                icons::ARCHIVE_MINIMALISTIC,
                "Archive Zapper and threads",
                2,
            ),
        ] {
            let z = zot.clone();
            manage = manage.child(list_row(
                key,
                glyph,
                theme.text_muted,
                label.to_owned(),
                None,
                theme,
                move |this, _, _, cx| {
                    let (action, after) = match kind {
                        0 => (ZotAction::FreshConversation, ZotAfter::OpenCoordinator),
                        1 => (
                            ZotAction::SetStatus {
                                status: ZotStatus::Stopped,
                            },
                            ZotAfter::LeavePage,
                        ),
                        _ => (
                            ZotAction::SetStatus {
                                status: ZotStatus::Archived,
                            },
                            ZotAfter::LeavePage,
                        ),
                    };
                    this.send_zot_command(z.id.clone(), Some(z.revision), action, after, cx)
                },
                cx,
            ));
        }
        let z = zot.clone();
        if self.zot_ui.confirm_delete {
            manage = manage
                .child(muted(
                    "Delete this Zapper? Its threads and documents are kept.",
                    theme,
                ))
                .child(list_row(
                    "zot-delete-confirm",
                    icons::TRASH_BIN_MINIMALISTIC,
                    theme.danger,
                    "Delete Zapper".to_owned(),
                    None,
                    theme,
                    move |this, _, _, cx| {
                        this.zot_ui.confirm_delete = false;
                        this.send_zot_command(
                            z.id.clone(),
                            Some(z.revision),
                            ZotAction::Delete,
                            ZotAfter::LeavePage,
                            cx,
                        );
                    },
                    cx,
                ));
        } else {
            manage = manage.child(list_row(
                "zot-delete",
                icons::TRASH_BIN_MINIMALISTIC,
                theme.danger,
                "Delete…".to_owned(),
                None,
                theme,
                |this, _, _, cx| {
                    this.zot_ui.confirm_delete = true;
                    cx.notify();
                },
                cx,
            ));
        }
        manage.into_any_element()
    }

    fn zot_threads(&mut self, zot: &Zot, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let workers: Vec<&ZotWorker> = zot
            .workers
            .iter()
            .filter(|w| !self.zot_thread_archived(&w.chat_id, cx))
            .collect();
        let new_zot = zot.clone();
        let mut threads = div().flex().flex_col().gap(px(2.0)).child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .child(section(format!("Threads · {}", workers.len()), theme))
                .child(action(
                    "zot-new-worker",
                    "New thread",
                    theme,
                    move |this, _, _, cx| {
                        this.open_zot_dialog(new_zot.clone(), Kind::Thread(None), cx)
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
            let status = match self.zot_thread_status(&id, cx) {
                Some(S::Working) => "Working",
                Some(S::AwaitingInput) => "Waiting for input",
                Some(S::Errored) => "Failed",
                Some(S::Idle) => "Idle",
                None => "Not running",
            };
            let dot = self.zot_status_color(&id, theme, cx);
            let open = id.clone();
            let row_group = SharedString::from(format!("zot-thread-row-{id}"));
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
            let z = zot.clone();
            let message = id.clone();
            actions = actions.child(thread_action(
                format!("zot-message-{id}"),
                icons::CHAT_ROUND_LINE,
                "Send a message",
                theme,
                move |this, _, _, cx| {
                    this.open_zot_dialog(z.clone(), Kind::Thread(Some(message.clone())), cx)
                },
                cx,
            ));
            for (label, glyph, kind) in [
                ("Stop", icons::STOP, 0),
                ("Release from Zapper", icons::ARROW_UP_RIGHT, 2),
                ("Archive", icons::ARCHIVE_MINIMALISTIC, 1),
            ] {
                let chat_id = id.clone();
                let z = zot.clone();
                actions = actions.child(thread_action(
                    format!("zot-worker-{kind}-{id}"),
                    glyph,
                    label,
                    theme,
                    move |this, _, _, cx| {
                        let chat_id = chat_id.clone();
                        if kind == 1 {
                            this.archive_zot_thread(&z, chat_id, cx);
                            return;
                        }
                        let action = if kind == 0 {
                            ZotAction::StopWorker { chat_id }
                        } else {
                            ZotAction::ReleaseWorker { chat_id }
                        };
                        this.send_zot_command(
                            z.id.clone(),
                            Some(z.revision),
                            action,
                            ZotAfter::Stay,
                            cx,
                        );
                    },
                    cx,
                ));
            }
            threads = threads.child(
                div()
                    .id(SharedString::from(format!("zot-open-{id}")))
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
        let mut coordinator = div()
            .flex()
            .flex_col()
            .gap(px(2.0))
            .child(section("Coordinator conversations", theme));
        let current = zot.coordinator_chat_id.clone();
        coordinator = coordinator.child(list_row(
            "zot-current-conversation",
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
        for (index, id) in zot.previous_coordinator_chats.iter().enumerate().rev() {
            let id = id.clone();
            coordinator = coordinator.child(list_row(
                format!("zot-history-{id}"),
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
        let candidates: Vec<_> = {
            let state = self.state.read(cx);
            state
                .chats
                .iter()
                .filter(|chat| {
                    chat.parent_chat_id.is_none()
                        && !chat.archived
                        && !zot_owns_chat(&state.zots, &chat.id)
                        && zot.config.repositories.iter().any(|r| {
                            r.device_id == chat.device_id
                                && Some(&r.space_id) == chat.space_id.as_ref()
                        })
                })
                .take(16)
                .cloned()
                .collect()
        };
        if !candidates.is_empty() {
            let mut adopt = div()
                .flex()
                .flex_col()
                .gap(px(2.0))
                .child(section("Add an existing thread", theme));
            for chat in candidates {
                let z = zot.clone();
                let title = chat.title.clone().unwrap_or_else(|| chat.id.clone());
                adopt = adopt.child(list_row(
                    format!("zot-adopt-{}", chat.id),
                    icons::PLUS,
                    theme.text_faint,
                    title.clone(),
                    None,
                    theme,
                    move |this, _, _, cx| {
                        this.send_zot_command(
                            z.id.clone(),
                            Some(z.revision),
                            ZotAction::AdoptWorker {
                                worker: ZotWorker {
                                    chat_id: chat.id.clone(),
                                    device_id: chat.device_id.clone(),
                                    space_id: chat.space_id.clone().unwrap_or_default(),
                                    objective: title.clone(),
                                    followups: 0,
                                },
                            },
                            ZotAfter::Stay,
                            cx,
                        );
                    },
                    cx,
                ));
            }
            view = view.child(adopt);
        }
        view.into_any_element()
    }

    fn zot_drive(
        &mut self,
        zot: &Zot,
        files: Vec<ZotFile>,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let files: Vec<_> = files.into_iter().filter(is_document).collect();
        let mut view = div()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .child(section(format!("Drive · {}", files.len()), theme));
        if files.is_empty() {
            view = view.child(muted(
                if self.zot_ui.loading {
                    "Loading documents…"
                } else {
                    "Rules, notes and the digest this Zapper keeps live here."
                },
                theme,
            ));
        }
        for file in files {
            let z = zot.clone();
            let preview: String = file.content.lines().take(6).collect::<Vec<_>>().join("\n");
            view = view.child(
                div()
                    .id(SharedString::from(format!("zot-edit-{}", file.path)))
                    .role(gpui::Role::Button)
                    .aria_label(SharedString::from(file.path.clone()))
                    .p(px(10.0))
                    .flex()
                    .flex_col()
                    .gap(px(6.0))
                    .rounded(px(8.0))
                    .border_1()
                    .border_color(theme.border)
                    .bg(theme.surface_card)
                    .cursor_pointer()
                    .hover(|d| d.border_color(theme.border_strong))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .child(
                                icon(icons::DOCUMENT)
                                    .size(px(14.0))
                                    .text_color(theme.text_muted),
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
                                    .child(format!("v{}", file.version)),
                            ),
                    )
                    .when(!preview.trim().is_empty(), |d| {
                        d.child(
                            div()
                                .text_size(crate::typography::ui_rems(11.5))
                                .text_color(theme.text_muted)
                                .child(preview),
                        )
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.open_zot_dialog(z.clone(), Kind::File(file.clone()), cx)
                    })),
            );
        }
        view.into_any_element()
    }

    fn zot_history(
        &mut self,
        zot: &Zot,
        files: Vec<ZotFile>,
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
                if self.zot_ui.loading {
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
            let z = zot.clone();
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
                            format!("zot-restore-{}-{}", file.path, file.version),
                            "Restore",
                            theme,
                            move |this, _, _, cx| {
                                this.send_zot_command(
                                    z.id.clone(),
                                    Some(z.revision),
                                    ZotAction::RestoreFile {
                                        path: file.path.clone(),
                                        version: file.version,
                                        expected_version: current,
                                    },
                                    ZotAfter::Stay,
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

    fn file(path: &str, version: u32, content: &str) -> ZotFile {
        ZotFile {
            path: path.into(),
            version,
            content: content.into(),
        }
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
    fn empty_drive_and_history_tabs_stay_hidden() {
        let seeded = [
            file("INSTRUCTIONS.md", 1, "Ship the release"),
            file("NOTES.md", 1, ""),
        ];
        assert_eq!(
            visible_tabs(&seeded, None),
            [ZotTab::Overview, ZotTab::Issues, ZotTab::Threads]
        );
        assert!(visible_tabs(&[], Some(ZotTab::History)).contains(&ZotTab::History));
        let written = [file("NOTES.md", 2, "Decided on Tuesday")];
        assert_eq!(visible_tabs(&written, None), ZotTab::ALL);
    }
}
