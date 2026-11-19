//! Linking a chat to a Slack thread from a right-click menu. The chat menu
//! links the chat itself; the Zapper menu links its coordinator chat (the
//! engine posts those replies under the Zapper's own name and avatar).
//!
//! Also holds the hover-submenu machinery both menus share: the chat menu's
//! Copy and Slack submenus and the Zapper menu's Slack submenu all open beside
//! their row on hover, with the same grace corridor toward the open submenu.
//! Clicking the Slack row pins its submenu open so its channel search can be
//! typed into without the pointer having to stay inside the submenu.
use super::*;
use std::collections::{HashMap, HashSet};
use zuse_protocol::slack::{Channel, ChatLink};

/// Hover-intent key of the Zapper menu's Slack row (its only submenu).
pub(super) const ZOT_MENU_ROW_SLACK: u8 = 3;
/// A row key that owns no submenu: switching to it closes any open one.
const NO_SUBMENU: u8 = u8::MAX;
const COPY_SUBMENU_WIDTH: f32 = 232.0;
const SLACK_SUBMENU_WIDTH: f32 = 264.0;

/// The context menu a submenu hangs off.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum SubmenuHost {
    Chat,
    Zot,
}

/// Pointer intent and geometry of a context menu's open submenu.
#[derive(Default)]
pub(super) struct Submenu {
    pub(super) intent: popover::HoverIntent<u8>,
    /// The open submenu's painted bounds: presses inside it are inside the menu.
    pub(super) bounds: Option<gpui::Bounds<Pixels>>,
    /// Opens to the left of the menu when the right side has no room.
    pub(super) left: bool,
    /// Clicked open: leaving it no longer closes it, so its search can be used.
    pub(super) pinned: bool,
}

impl Submenu {
    pub(super) fn reset(&mut self) {
        self.intent.reset();
        self.pinned = false;
    }
}

/// What the engine reported for one chat.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct LinkState {
    pub(super) link: Option<ChatLink>,
    /// False until Slack is connected in Settings → Cloud.
    pub(super) connected: bool,
}

/// The short status shown at the end of a menu's Slack row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SlackStatus {
    On,
    Off,
    NotLinked,
}

/// Nothing while the link is unknown (loading or failed).
pub(super) fn slack_status(state: Option<&Loadable<LinkState>>) -> Option<SlackStatus> {
    match &state?.ready()?.link {
        Some(link) if link.sync => Some(SlackStatus::On),
        Some(_) => Some(SlackStatus::Off),
        None => Some(SlackStatus::NotLinked),
    }
}

pub(super) fn parse_link_state(data: &serde_json::Value) -> Result<LinkState, String> {
    let link: Option<ChatLink> =
        serde_json::from_value(data.get("link").cloned().unwrap_or_default())
            .map_err(|e| e.to_string())?;
    let connected = link.is_some() || data["connected"].as_bool().unwrap_or(false);
    Ok(LinkState { link, connected })
}

/// Channels matching a search, best matches first. A leading `#` is ignored.
pub(super) fn filter_channels(query: &str, channels: &[Channel]) -> Vec<usize> {
    let query = query.trim().trim_start_matches('#');
    let names: Vec<&str> = channels.iter().map(|c| c.name.as_str()).collect();
    popover::filter_indices(query, &names)
}

/// Team names only help when channels come from more than one workspace.
fn spans_teams(channels: &[Channel]) -> bool {
    channels
        .iter()
        .map(|c| c.team_id.as_str())
        .collect::<HashSet<_>>()
        .len()
        > 1
}

/// Session cache of chat links and channels, refreshed each time a menu opens.
#[derive(Default)]
pub(super) struct SlackLinks {
    links: HashMap<String, Loadable<LinkState>>,
    /// Chats whose link was requested since their menu last opened.
    fresh: HashSet<String>,
    link_tasks: HashMap<String, Task<()>>,
    channels: Loadable<Vec<Channel>>,
    channels_fresh: bool,
    channels_task: Option<Task<()>>,
    /// The chat a link/sync/unlink change is in flight for.
    busy: Option<String>,
    mutation: Option<Task<()>>,
    search: Option<(Entity<ComposerInput>, Subscription)>,
    scroll: gpui::ScrollHandle,
}

impl SlackLinks {
    /// Keep what was loaded so the next menu shows it at once, but reload it.
    pub(super) fn menu_closed(&mut self) {
        self.fresh.clear();
        self.channels_fresh = false;
        self.search = None;
    }
}

struct Trigger {
    host: SubmenuHost,
    key: u8,
    id: &'static str,
    chat_id: String,
    glyph: &'static str,
    label: SharedString,
    status: Option<SharedString>,
    open: bool,
    width: f32,
    pin_on_click: bool,
}

impl Shell {
    fn submenu(&mut self, host: SubmenuHost) -> &mut Submenu {
        match host {
            SubmenuHost::Chat => &mut self.chat_submenu,
            SubmenuHost::Zot => &mut self.zot_ui.submenu,
        }
    }

    fn submenu_ref(&self, host: SubmenuHost) -> &Submenu {
        match host {
            SubmenuHost::Chat => &self.chat_submenu,
            SubmenuHost::Zot => &self.zot_ui.submenu,
        }
    }

    fn host_menu_open(&self, host: SubmenuHost) -> bool {
        match host {
            SubmenuHost::Chat => self.chat_menu.is_open(),
            SubmenuHost::Zot => self.zot_ui.menu.is_open(),
        }
    }

    /// The row key whose submenu is open, if any.
    fn open_submenu_key(&self, host: SubmenuHost) -> Option<u8> {
        match host {
            SubmenuHost::Chat => self.chat_menu.as_open().and_then(|menu| match menu.page {
                ChatMenuPage::Root => None,
                ChatMenuPage::Copy => Some(CHAT_MENU_ROW_COPY),
                ChatMenuPage::Slack => Some(CHAT_MENU_ROW_SLACK),
            }),
            SubmenuHost::Zot => self
                .zot_ui
                .menu
                .as_open()
                .filter(|_| self.zot_ui.slack_open)
                .map(|_| ZOT_MENU_ROW_SLACK),
        }
    }

    /// Show the submenu row `key` owns; rows without one close any open submenu.
    fn set_submenu(&mut self, host: SubmenuHost, key: u8, cx: &mut Context<Self>) {
        let changed = match host {
            SubmenuHost::Chat => {
                let page = match key {
                    CHAT_MENU_ROW_COPY => ChatMenuPage::Copy,
                    CHAT_MENU_ROW_SLACK => ChatMenuPage::Slack,
                    _ => ChatMenuPage::Root,
                };
                match self.chat_menu.open_mut() {
                    Some(menu) if menu.page != page => {
                        menu.page = page;
                        true
                    }
                    _ => false,
                }
            }
            SubmenuHost::Zot => {
                let open = key == ZOT_MENU_ROW_SLACK;
                if self.zot_ui.menu.is_open() && self.zot_ui.slack_open != open {
                    self.zot_ui.slack_open = open;
                    true
                } else {
                    false
                }
            }
        };
        if changed {
            let submenu = self.submenu(host);
            submenu.pinned = false;
            submenu.bounds = None;
            cx.notify();
        }
    }

    fn close_submenu_host(&mut self, host: SubmenuHost, cx: &mut Context<Self>) {
        match host {
            SubmenuHost::Chat => self.close_chat_menu(cx),
            SubmenuHost::Zot => self.close_zot_menu(cx),
        }
    }

    /// Pointer intent over a menu row: hovering a row with a submenu opens it,
    /// any other row closes it — after a grace while the pointer is heading
    /// into the open submenu. A pinned submenu stays until another row is clicked.
    fn hover_submenu_row(
        &mut self,
        host: SubmenuHost,
        key: u8,
        pointer: Point<Pixels>,
        moved: bool,
        cx: &mut Context<Self>,
    ) {
        let current = self.open_submenu_key(host);
        if self.submenu_ref(host).pinned && current.is_some() {
            return;
        }
        let submenu = self.submenu(host);
        let (bounds, left) = (submenu.bounds, submenu.left);
        let action = if moved {
            submenu
                .intent
                .moved(current.as_ref(), &key, pointer, bounds, left)
        } else {
            submenu
                .intent
                .enter(current.as_ref(), &key, pointer, bounds, left)
        };
        match action {
            popover::HoverAction::None => {}
            popover::HoverAction::Open => {
                self.set_submenu(host, key, cx);
                self.submenu(host).intent.record_origin(pointer);
            }
            popover::HoverAction::Defer => {
                self.submenu(host).intent.defer(cx, move |this, cx| {
                    if this.host_menu_open(host)
                        && this.submenu_ref(host).intent.pending() == Some(&key)
                    {
                        this.submenu(host).intent.cancel();
                        this.set_submenu(host, key, cx);
                        this.submenu(host).intent.record_origin(pointer);
                    }
                });
            }
        }
    }

    /// Wrap a menu row in the hover surface that feeds
    /// [`Self::hover_submenu_row`]. The row keeps its own hover fade.
    pub(super) fn submenu_hover_row(
        &self,
        host: SubmenuHost,
        key: u8,
        row: impl IntoElement,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = match host {
            SubmenuHost::Chat => "chat-menu-hover",
            SubmenuHost::Zot => "zot-menu-hover",
        };
        div()
            .id((id, key as usize))
            .on_hover(cx.listener(move |this, hovered: &bool, window, cx| {
                if *hovered {
                    this.hover_submenu_row(host, key, window.mouse_position(), false, cx);
                } else {
                    this.submenu(host).intent.leave(&key);
                }
            }))
            .on_mouse_move(
                cx.listener(move |this, event: &gpui::MouseMoveEvent, _, cx| {
                    this.hover_submenu_row(host, key, event.position, true, cx);
                }),
            )
            .child(row)
            .into_any_element()
    }

    pub(super) fn chat_menu_hover_row(
        &self,
        key: u8,
        row: impl IntoElement,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.submenu_hover_row(SubmenuHost::Chat, key, row, cx)
    }

    /// A row that opens its submenu beside the menu on hover (or click), on
    /// whichever side has room.
    fn render_submenu_trigger(
        &mut self,
        trigger: Trigger,
        submenu: Option<AnyElement>,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Trigger {
            host,
            key,
            id,
            chat_id,
            glyph,
            label,
            status,
            open,
            width,
            pin_on_click,
        } = trigger;
        let entity = cx.entity().downgrade();
        let exit_entity = entity.clone();
        let mut row = popover::menu_row_nav(theme, open, false, format!("{id}-{chat_id}"))
            .id(id)
            .relative()
            .on_click(cx.listener(move |this, _, window, cx| {
                this.submenu(host).intent.reset();
                this.set_submenu(host, key, cx);
                if pin_on_click {
                    this.submenu(host).pinned = true;
                    this.focus_slack_search(window, cx);
                }
                cx.stop_propagation();
            }))
            .child(icon(glyph).size(px(16.0)).text_color(theme.text_muted))
            .child(div().flex_1().child(label))
            .when_some(status, |row, status| {
                row.child(
                    div()
                        .text_size(crate::typography::text_xs())
                        .text_color(theme.text_muted)
                        .child(status),
                )
            })
            .child(
                icon(icons::ALT_ARROW_RIGHT)
                    .size(px(14.0))
                    .text_color(theme.text_muted),
            )
            .child(
                gpui::canvas(
                    move |bounds, window, cx| {
                        let left = bounds.right() + px(width + 12.0) > window.viewport_size().width;
                        let _ = entity.update(cx, |this, cx| {
                            if this.submenu_ref(host).left != left {
                                this.submenu(host).left = left;
                                cx.notify();
                            }
                        });
                    },
                    move |trigger, _, window, _| {
                        if !open {
                            return;
                        }
                        // Leaving the trigger anywhere but toward the submenu
                        // closes it, as in the sidebar options menu.
                        window.on_mouse_event(move |event: &gpui::MouseMoveEvent, phase, _, cx| {
                            if phase != gpui::DispatchPhase::Bubble {
                                return;
                            }
                            let _ = exit_entity.update(cx, |this, cx| {
                                let submenu = this.submenu(host);
                                if submenu.pinned {
                                    return;
                                }
                                let (bounds, left) = (submenu.bounds, submenu.left);
                                if submenu.intent.contains_pointer(
                                    trigger,
                                    bounds,
                                    event.position,
                                    left,
                                ) {
                                    return;
                                }
                                submenu.intent.reset();
                                this.set_submenu(host, NO_SUBMENU, cx);
                            });
                        });
                    },
                )
                .absolute()
                .inset_0(),
            );
        if let Some(submenu) = submenu {
            let left = self.submenu_ref(host).left;
            row = row.child(popover::nested_menu(
                format!("{id}-submenu-{chat_id}"),
                submenu,
                left,
            ));
        }
        row.into_any_element()
    }

    /// The chat menu's Copy row and, when open, its submenu.
    pub(super) fn render_chat_copy_trigger(
        &mut self,
        chat_id: &str,
        open: bool,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let submenu = open.then(|| self.render_chat_copy_submenu(chat_id, theme, cx));
        self.render_submenu_trigger(
            Trigger {
                host: SubmenuHost::Chat,
                key: CHAT_MENU_ROW_COPY,
                id: "chat-menu-copy",
                chat_id: chat_id.to_owned(),
                glyph: icons::COPY,
                label: "Copy".into(),
                status: None,
                open,
                width: COPY_SUBMENU_WIDTH,
                pin_on_click: false,
            },
            submenu,
            theme,
            cx,
        )
    }

    /// A menu's Slack row: the chat's link status, and the Slack submenu.
    pub(super) fn render_slack_trigger(
        &mut self,
        host: SubmenuHost,
        chat_id: &str,
        open: bool,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.ensure_slack_link(chat_id, cx);
        let status = slack_status(self.slack_links.links.get(chat_id)).map(|status| {
            SharedString::from(match status {
                SlackStatus::On => tr(cx, msg::shell::slack_link_status_on),
                SlackStatus::Off => tr(cx, msg::shell::slack_link_status_off),
                SlackStatus::NotLinked => tr(cx, msg::shell::slack_link_status_not_linked),
            })
        });
        let submenu = open.then(|| self.render_slack_submenu(host, chat_id, theme, cx));
        let key = match host {
            SubmenuHost::Chat => CHAT_MENU_ROW_SLACK,
            SubmenuHost::Zot => ZOT_MENU_ROW_SLACK,
        };
        let id = match host {
            SubmenuHost::Chat => "chat-menu-slack",
            SubmenuHost::Zot => "zot-menu-slack",
        };
        self.render_submenu_trigger(
            Trigger {
                host,
                key,
                id,
                chat_id: chat_id.to_owned(),
                glyph: icons::CHAT_ROUND_LINE,
                label: tr(cx, msg::shell::slack_link_row).into(),
                status,
                open,
                width: SLACK_SUBMENU_WIDTH,
                pin_on_click: true,
            },
            submenu,
            theme,
            cx,
        )
    }

    /// Run a Slack RPC on the engine hosting local chats; `apply` gets the
    /// response's `data`.
    fn spawn_slack_rpc(
        &mut self,
        method: &'static str,
        params: serde_json::Value,
        cx: &mut Context<Self>,
        apply: impl FnOnce(&mut Self, Result<serde_json::Value, String>, &mut Context<Self>) + 'static,
    ) -> Task<()> {
        let engine = self.state.read(cx).engine().map(|e| e.controller());
        let unavailable = tr(cx, msg::shell::slack_link_unavailable);
        let job = engine.map(|engine| {
            Tokio::spawn(cx, async move {
                engine
                    .client()
                    .call_scoped(method, params)
                    .await
                    .map_err(|e| e.to_string())
            })
        });
        cx.spawn(async move |this, cx| {
            let result = match job {
                Some(job) => job.await.unwrap_or_else(|e| Err(e.to_string())),
                None => Err(unavailable),
            };
            let _ = this.update(cx, |this, cx| {
                apply(this, result.map(|value| value["data"].clone()), cx);
                cx.notify();
            });
        })
    }

    /// Load `chat_id`'s link once per menu opening.
    fn ensure_slack_link(&mut self, chat_id: &str, cx: &mut Context<Self>) {
        if chat_id.is_empty() || !self.slack_links.fresh.insert(chat_id.to_owned()) {
            return;
        }
        let entry = self
            .slack_links
            .links
            .entry(chat_id.to_owned())
            .or_default();
        if entry.ready().is_none() {
            *entry = Loadable::Loading;
        }
        let id = chat_id.to_owned();
        let task = self.spawn_slack_rpc(
            methods::SLACK_CHAT_LINK,
            serde_json::json!({ "chatId": chat_id }),
            cx,
            move |this, result, _| {
                this.slack_links.link_tasks.remove(&id);
                let next = match result.and_then(|data| parse_link_state(&data)) {
                    Ok(state) => Loadable::Ready(state),
                    // A refresh failing keeps what is already shown.
                    Err(_)
                        if this
                            .slack_links
                            .links
                            .get(&id)
                            .is_some_and(|s| s.ready().is_some()) =>
                    {
                        return;
                    }
                    Err(error) => Loadable::Error(error),
                };
                this.slack_links.links.insert(id, next);
            },
        );
        self.slack_links.link_tasks.insert(chat_id.to_owned(), task);
    }

    /// Load the channels a chat can be linked to, once per menu opening.
    fn ensure_slack_channels(&mut self, cx: &mut Context<Self>) {
        if std::mem::replace(&mut self.slack_links.channels_fresh, true) {
            return;
        }
        if self.slack_links.channels.ready().is_none() {
            self.slack_links.channels = Loadable::Loading;
        }
        let task = self.spawn_slack_rpc(
            methods::SLACK_CHANNELS,
            serde_json::json!({}),
            cx,
            |this, result, _| {
                this.slack_links.channels_task = None;
                let parsed = result.and_then(|data| {
                    serde_json::from_value::<Vec<Channel>>(data["channels"].clone())
                        .map_err(|e| e.to_string())
                });
                match parsed {
                    Ok(channels) => this.slack_links.channels = Loadable::Ready(channels),
                    Err(_) if this.slack_links.channels.ready().is_some() => {}
                    Err(error) => this.slack_links.channels = Loadable::Error(error),
                }
            },
        );
        self.slack_links.channels_task = Some(task);
    }

    /// Link, toggle or unlink; the response replaces the chat's link.
    fn change_slack_link(
        &mut self,
        chat_id: &str,
        method: &'static str,
        params: serde_json::Value,
        cx: &mut Context<Self>,
    ) {
        if self.slack_links.busy.is_some() {
            return;
        }
        self.slack_links.busy = Some(chat_id.to_owned());
        let id = chat_id.to_owned();
        let task = self.spawn_slack_rpc(method, params, cx, move |this, result, cx| {
            this.slack_links.busy = None;
            this.slack_links.mutation = None;
            let link = result.and_then(|data| {
                if method == methods::SLACK_CHAT_LINK_REMOVE {
                    Ok(None)
                } else {
                    serde_json::from_value::<ChatLink>(data["link"].clone())
                        .map(Some)
                        .map_err(|e| e.to_string())
                }
            });
            match link {
                Ok(link) => {
                    this.slack_links.links.insert(
                        id,
                        Loadable::Ready(LinkState {
                            link,
                            connected: true,
                        }),
                    );
                }
                Err(error) => this.show_notice(crate::toast::ToastKind::Error, error, cx),
            }
        });
        self.slack_links.mutation = Some(task);
        cx.notify();
    }

    fn slack_search(&mut self, cx: &mut Context<Self>) -> Entity<ComposerInput> {
        if let Some((search, _)) = &self.slack_links.search {
            return search.clone();
        }
        let placeholder = tr(cx, msg::shell::slack_link_search_placeholder);
        // "PaletteSearch": only text-editing keys bind inside the input.
        let search = cx.new(|cx| ComposerInput::with_context(placeholder, "PaletteSearch", cx));
        let events = cx.subscribe(&search, |_, _, event: &ComposerInputEvent, cx| {
            if matches!(event, ComposerInputEvent::Edited) {
                cx.notify();
            }
        });
        self.slack_links.search = Some((search.clone(), events));
        search
    }

    /// Clicking the Slack row of an unlinked chat puts the caret in the search.
    fn focus_slack_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let chat_id = match self.chat_menu.as_open() {
            Some(menu) if self.open_submenu_key(SubmenuHost::Chat).is_some() => {
                Some(menu.chat_id.clone())
            }
            _ => self.zot_ui.menu.as_open().and_then(|(zot_id, _)| {
                self.state
                    .read(cx)
                    .zots
                    .iter()
                    .find(|z| &z.id == zot_id)
                    .map(|z| z.coordinator_chat_id.clone())
            }),
        };
        let picking = chat_id
            .and_then(|id| self.slack_links.links.get(&id))
            .and_then(|state| state.ready())
            .is_some_and(|state| state.connected && state.link.is_none());
        if picking {
            let handle = self.slack_search(cx).read(cx).focus_handle(cx);
            window.focus(&handle, cx);
        }
    }

    /// The Slack submenu's card for `chat_id`.
    fn render_slack_submenu(
        &mut self,
        host: SubmenuHost,
        chat_id: &str,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let muted = |text: String| {
            div()
                .px(px(8.0))
                .py(px(6.0))
                .text_size(crate::typography::text_sm())
                .text_color(theme.text_muted)
                .child(SharedString::from(text))
        };
        let settings_row = popover::menu_row(theme, false, format!("slack-settings-{chat_id}"))
            .id("slack-link-settings")
            .child(
                icon(icons::SETTINGS)
                    .size(px(16.0))
                    .text_color(theme.text_muted),
            )
            .child(tr(cx, msg::shell::slack_link_settings))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.close_submenu_host(host, cx);
                this.open_settings(SettingsSection::Cloud, cx);
            }));
        let busy = self.slack_links.busy.as_deref() == Some(chat_id);
        let state = self
            .slack_links
            .links
            .get(chat_id)
            .cloned()
            .unwrap_or_default();
        let mut card = popover::popover_card(theme)
            .w(px(SLACK_SUBMENU_WIDTH))
            .relative()
            .flex()
            .flex_col()
            .gap(px(2.0))
            // Working inside the submenu (the search field) keeps it open.
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, _| this.submenu(host).pinned = true),
            );
        card = match state {
            Loadable::Idle | Loadable::Loading => {
                card.child(muted(tr(cx, msg::shell::slack_link_loading)))
            }
            Loadable::Error(error) => card
                .child(muted(error))
                .child(popover::menu_separator())
                .child(settings_row),
            Loadable::Ready(LinkState {
                link: Some(link), ..
            }) => {
                let heading = tr(cx, |l| {
                    msg::shell::slack_link_linked_heading(
                        l,
                        link.channel_name.clone(),
                        link.team_name.clone(),
                    )
                });
                let label = tr(cx, msg::shell::slack_link_sync);
                let sync = link.sync;
                let toggle_id = chat_id.to_owned();
                let unlink_id = chat_id.to_owned();
                let toggle = popover::menu_row(theme, false, format!("slack-sync-{chat_id}"))
                    .id("slack-link-sync")
                    .role(gpui::Role::Switch)
                    .aria_label(SharedString::from(label.clone()))
                    .aria_toggled(if sync {
                        gpui::Toggled::True
                    } else {
                        gpui::Toggled::False
                    })
                    .opacity(if busy { 0.5 } else { 1.0 })
                    .child(div().flex_1().child(label))
                    .child(crate::settings::widgets::toggle_switch(
                        theme,
                        sync,
                        format!("slack-sync-switch-{chat_id}"),
                    ))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.change_slack_link(
                            &toggle_id,
                            methods::SLACK_CHAT_LINK_UPDATE,
                            serde_json::json!({ "chatId": toggle_id, "sync": !sync }),
                            cx,
                        );
                    }));
                let open_thread = link.permalink.clone().map(|url| {
                    popover::menu_row(theme, false, format!("slack-open-{chat_id}"))
                        .id("slack-link-open")
                        .child(
                            icon(icons::ARROW_UP_RIGHT)
                                .size(px(16.0))
                                .text_color(theme.text_muted),
                        )
                        .child(tr(cx, msg::shell::slack_link_open_thread))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.close_submenu_host(host, cx);
                            cx.open_url(&url);
                        }))
                });
                let unlink = popover::menu_row(theme, false, format!("slack-unlink-{chat_id}"))
                    .id("slack-link-unlink")
                    .opacity(if busy { 0.5 } else { 1.0 })
                    .child(
                        icon(icons::CLOSE_CIRCLE)
                            .size(px(16.0))
                            .text_color(theme.text_muted),
                    )
                    .child(tr(cx, msg::shell::slack_link_unlink))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.change_slack_link(
                            &unlink_id,
                            methods::SLACK_CHAT_LINK_REMOVE,
                            serde_json::json!({ "chatId": unlink_id }),
                            cx,
                        );
                    }));
                card.child(popover::menu_heading(theme, &heading))
                    .child(toggle)
                    .children(open_thread)
                    .child(unlink)
                    .child(popover::menu_separator())
                    .child(settings_row)
            }
            Loadable::Ready(LinkState {
                connected: false, ..
            }) => card.child(
                popover::menu_row(theme, false, format!("slack-connect-{chat_id}"))
                    .id("slack-link-connect")
                    .child(
                        icon(icons::SETTINGS)
                            .size(px(16.0))
                            .text_color(theme.text_muted),
                    )
                    .child(tr(cx, msg::shell::slack_link_connect))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.close_submenu_host(host, cx);
                        this.open_settings(SettingsSection::Cloud, cx);
                    })),
            ),
            Loadable::Ready(_) => {
                self.ensure_slack_channels(cx);
                let search = self.slack_search(cx);
                let query = search.read(cx).text().to_owned();
                let list = self.render_slack_channels(chat_id, &query, busy, theme, cx);
                // The search row mounts first: it bleeds to the card's edges.
                card.child(popover::search_input_frame(
                    theme,
                    search.into_any_element(),
                ))
                .child(popover::menu_heading(
                    theme,
                    &tr(cx, msg::shell::slack_link_heading),
                ))
                .child(list)
                .child(popover::menu_separator())
                .child(settings_row)
            }
        };
        let entity = cx.entity().downgrade();
        card.child(
            gpui::canvas(
                move |bounds, _, cx| {
                    let _ = entity.update(cx, |this, _| this.submenu(host).bounds = Some(bounds));
                },
                |_, _, _, _| {},
            )
            .absolute()
            .inset_0(),
        )
        .into_any_element()
    }

    fn render_slack_channels(
        &mut self,
        chat_id: &str,
        query: &str,
        busy: bool,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let muted = |text: String| {
            div()
                .px(px(8.0))
                .py(px(6.0))
                .text_size(crate::typography::text_sm())
                .text_color(theme.text_muted)
                .child(SharedString::from(text))
                .into_any_element()
        };
        let channels = match &self.slack_links.channels {
            Loadable::Idle | Loadable::Loading => {
                return muted(tr(cx, msg::shell::slack_link_loading));
            }
            Loadable::Error(error) => return muted(error.clone()),
            Loadable::Ready(channels) => channels.clone(),
        };
        let rows = filter_channels(query, &channels);
        if rows.is_empty() {
            return muted(if channels.is_empty() {
                tr(cx, msg::shell::slack_link_no_channels)
            } else {
                tr(cx, msg::shell::slack_link_no_matching_channels)
            });
        }
        let teams = spans_teams(&channels);
        let private = tr(cx, msg::shell::slack_link_private);
        let scroll = self.slack_links.scroll.clone();
        let list = popover::menu_scroll_list("slack-channel-list", &scroll)
            .flex()
            .flex_col()
            .gap(px(2.0))
            .max_h(px(240.0))
            .children(rows.into_iter().map(|ix| {
                let channel = channels[ix].clone();
                let trailing = match (teams, channel.private) {
                    (true, true) => Some(format!("{} · {private}", channel.team_name)),
                    (true, false) => Some(channel.team_name.clone()),
                    (false, true) => Some(private.clone()),
                    (false, false) => None,
                };
                let name = channel.name.clone();
                let link_id = chat_id.to_owned();
                popover::menu_row(
                    theme,
                    false,
                    format!("slack-channel-{}-{}", channel.team_id, channel.channel_id),
                )
                .id(SharedString::from(format!(
                    "slack-channel-{}-{}",
                    channel.team_id, channel.channel_id
                )))
                .gap(px(6.0))
                .opacity(if busy { 0.5 } else { 1.0 })
                .child(div().flex_none().text_color(theme.text_muted).child("#"))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .child(SharedString::from(name)),
                )
                .when_some(trailing, |row, trailing| {
                    row.child(
                        div()
                            .flex_none()
                            .max_w(px(110.0))
                            .truncate()
                            .text_size(crate::typography::text_xs())
                            .text_color(theme.text_muted)
                            .child(SharedString::from(trailing)),
                    )
                })
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.change_slack_link(
                        &link_id,
                        methods::SLACK_CHAT_LINK_CREATE,
                        serde_json::json!({
                            "chatId": link_id,
                            "teamId": channel.team_id,
                            "channelId": channel.channel_id,
                        }),
                        cx,
                    );
                }))
            }));
        popover::menu_scroll_host("slack-channel-list-host")
            .child(list)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn link(sync: bool) -> ChatLink {
        ChatLink {
            team_id: "T1".into(),
            team_name: "Acme".into(),
            channel_id: "C1".into(),
            channel_name: "general".into(),
            thread_ts: "1.0".into(),
            sync,
            permalink: None,
        }
    }

    fn channel(team: &str, name: &str) -> Channel {
        Channel {
            team_id: team.into(),
            team_name: team.into(),
            channel_id: name.into(),
            name: name.into(),
            private: false,
        }
    }

    #[test]
    fn status_follows_the_link_and_stays_blank_until_known() {
        let ready = |link| {
            Loadable::Ready(LinkState {
                link,
                connected: true,
            })
        };
        assert_eq!(slack_status(None), None);
        assert_eq!(slack_status(Some(&Loadable::Loading)), None);
        assert_eq!(slack_status(Some(&Loadable::Error("x".into()))), None);
        assert_eq!(
            slack_status(Some(&ready(Some(link(true))))),
            Some(SlackStatus::On)
        );
        assert_eq!(
            slack_status(Some(&ready(Some(link(false))))),
            Some(SlackStatus::Off)
        );
        assert_eq!(
            slack_status(Some(&ready(None))),
            Some(SlackStatus::NotLinked)
        );
    }

    #[test]
    fn link_responses_parse_with_and_without_a_link() {
        let none =
            parse_link_state(&serde_json::json!({ "link": null, "connected": false })).unwrap();
        assert_eq!(
            none,
            LinkState {
                link: None,
                connected: false
            }
        );
        let some = parse_link_state(&serde_json::json!({
            "link": {
                "teamId": "T1", "teamName": "Acme", "channelId": "C1",
                "channelName": "general", "threadTs": "1.0", "sync": true
            },
            "connected": true
        }))
        .unwrap();
        assert_eq!(some.link, Some(link(true)));
        assert!(some.connected);
    }

    #[test]
    fn channel_search_ranks_prefixes_and_ignores_a_leading_hash() {
        let channels = [
            channel("T1", "eng-general"),
            channel("T1", "general"),
            channel("T2", "random"),
        ];
        assert_eq!(filter_channels("", &channels), vec![0, 1, 2]);
        assert_eq!(filter_channels("#gen", &channels), vec![1, 0]);
        assert_eq!(filter_channels("zzz", &channels), Vec::<usize>::new());
        assert!(spans_teams(&channels));
        assert!(!spans_teams(&channels[..2]));
    }
}
