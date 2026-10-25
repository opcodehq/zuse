//! Saved cloud history uses a separate viewport with no execution transport.
use super::*;
use crate::{
    assets::icon,
    settings_style as s,
    shell::{BORDER, MUTED, column, row},
};
use zuse_model::cloud::{ArchivedCloud, Head, Workspace};
const LIST: &str = zuse_protocol::methods::CLOUD_WORKSPACES;
const READ: &str = zuse_protocol::methods::CLOUD_HISTORY;
const CLEANUP_MIRROR: &str = zuse_protocol::methods::CLOUD_CLEANUP_MIRROR;
const DELETE: &str = zuse_protocol::methods::CLOUD_DELETE;
const UNARCHIVE: &str = zuse_protocol::methods::CLOUD_UNARCHIVE;
#[derive(Default)]
pub(super) struct ArchiveView {
    pub state: ArchivedCloud,
    owner: Option<String>,
    loaded: bool,
    list: Option<u64>,
    read: Option<(u64, u64)>,
    action: Option<(u64, String, bool)>,
    confirm_delete: Option<String>,
    confirm_focus: Option<[FocusHandle; 3]>,
    poll: Option<Task<()>>,
    view: Option<Entity<WorkspaceView>>,
    commands: HashMap<String, String>,
}
impl WorkspaceView {
    pub(super) fn interrupt_cloud_archive(&mut self) {
        self.cloud_archive.poll = None;
        self.cloud_archive.list = None;
        self.cloud_archive.read = None;
        self.cloud_archive.loaded = false;
        self.cloud_archive.state.loading = false;
        self.cloud_archive.state.busy = false;
        if self.cloud_archive.action.take().is_some() {
            self.cloud_archive.state.error = Some(
                "Connection interrupted. Refresh to check the workspace's current state.".into(),
            );
        }
    }

    pub(super) fn load_cloud_archive(&mut self) {
        if !self.cached_only
            && self.connected
            && self.cloud_archive.owner.is_some()
            && !self.cloud_archive.loaded
            && self.cloud_archive.list.is_none()
        {
            self.cloud_archive.list = Some(self.call(LIST, json!({})));
        }
    }
    pub(super) fn receive_cloud_archive(&mut self, frame: &Value, cx: &mut Context<Self>) -> bool {
        let Some(id) = frame["id"].as_u64() else {
            return false;
        };
        if self
            .pending
            .get(&id)
            .is_some_and(|m| m == methods::CLOUD_ACCOUNT_STATUS)
        {
            if let Some(value) = frame.get("item").or_else(|| frame.get("ok")) {
                let owner = value["user"]["id"].as_str().filter(|_| value["orgId"].is_null()).map(str::to_owned);
                if owner != self.cloud_archive.owner {
                    let mut state = self.cloud_archive.state.clone();
                    state.clear();
                    self.cloud_archive = ArchiveView {
                        owner,
                        state,
                        ..Default::default()
                    };
                }
                cx.notify();
            }
            return false;
        }
        let list = self.cloud_archive.list == Some(id);
        let read = self.cloud_archive.read.filter(|(req, _)| *req == id);
        let action = self
            .cloud_archive
            .action
            .clone()
            .filter(|(req, _, _)| *req == id);
        if !list && read.is_none() && action.is_none() {
            return false;
        }
        self.pending.remove(&id);
        if list {
            self.cloud_archive.list = None;
            self.cloud_archive.loaded = true;
        }
        if read.is_some() {
            self.cloud_archive.read = None;
            self.cloud_archive.state.loading = false;
        }
        if action.is_some() {
            self.cloud_archive.action = None;
            self.cloud_archive.state.busy = false;
        }
        let value = &frame["ok"];
        let error = if frame.get("err").is_some() {
            Some(
                frame["err"]
                    .as_str()
                    .unwrap_or("Cloud request failed. Try again.")
                    .to_owned(),
            )
        } else if value["accountId"].as_str() != self.cloud_archive.owner.as_deref() {
            Some("Your account changed. Reopen this view.".into())
        } else {
            None
        };
        if let Some(error) = error {
            self.cloud_archive.state.error = Some(error);
        } else if list {
            match serde_json::from_value::<Vec<Workspace>>(value["workspaces"].clone()) {
                Ok(rows) => self
                    .cloud_archive
                    .state
                    .list(self.cloud_archive.owner.clone().unwrap(), rows),
                Err(_) => {
                    self.cloud_archive.state.error =
                        Some("Cloud list is invalid. Try again.".into())
                }
            }
        } else if let Some((_, revision)) = read {
            match serde_json::from_value::<Head>(value["head"].clone()) {
                Ok(head) => {
                    self.cloud_archive.state.receive(revision, head);
                }
                Err(_) => {
                    self.cloud_archive.state.error = Some("Saved cloud history is invalid.".into())
                }
            }
        } else if let Some((_, workspace, deleting)) = action {
            if deleting {
                if let Ok(row) = serde_json::from_value::<Workspace>(value["workspace"].clone()) {
                    self.cloud_archive.state.update_workspace(row);
                }
                self.cloud_archive.loaded = false;
                self.load_cloud_archive();
            } else {
                self.cloud_archive.state.restored(&workspace);
            }
            self.cloud_archive.view = None;
            self.cloud_archive.commands.remove(&workspace);
        }
        if self.cloud_archive.state.needs_poll() && self.cloud_archive.poll.is_none() {
            self.cloud_archive.poll = Some(cx.spawn(async move |this, cx| {
                cx.background_executor()
                    .timer(std::time::Duration::from_secs(2))
                    .await;
                let _ = this.update(cx, |this, cx| {
                    this.cloud_archive.poll = None;
                    this.cloud_archive.loaded = false;
                    this.load_cloud_archive();
                    cx.notify();
                });
            }));
        }
        cx.notify();
        true
    }
    pub(super) fn dismiss_cloud_delete(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.cloud_archive.confirm_delete.take().is_none() {
            return false;
        }
        self.cloud_archive.confirm_focus = None;
        window.focus(&self.settings_nav_focus, cx);
        cx.notify();
        true
    }
    pub(super) fn submit_cloud_delete(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(id) = self.cloud_archive.confirm_delete.clone() else {
            return false;
        };
        if self
            .cloud_archive
            .confirm_focus
            .as_ref()
            .is_some_and(|focus| focus[2].is_focused(window))
        {
            window.focus(&self.settings_nav_focus, cx);
            self.delete_cloud(&id, cx);
        } else {
            self.dismiss_cloud_delete(window, cx);
        }
        true
    }
    fn delete_cloud(&mut self, id: &str, cx: &mut Context<Self>) {
        self.mutate_cloud(id, DELETE, cx)
    }
    fn mutate_cloud(&mut self, id: &str, method: &'static str, cx: &mut Context<Self>) {
        if self.cloud_archive.action.is_some() {
            return;
        }
        self.cloud_archive.confirm_delete = None;
        let command = self
            .cloud_archive
            .commands
            .entry(format!("{method}:{id}"))
            .or_insert_with(|| uuid::Uuid::new_v4().to_string())
            .clone();
        self.cloud_archive.state.busy = true;
        self.cloud_archive.state.error = None;
        let req = self.call(method, json!({"workspaceId":id,"commandId":command}));
        self.cloud_archive.action = Some((req, id.into(), true));
        cx.notify();
    }
    fn open_cloud_archive(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.cloud_archive.read.is_some() || self.cloud_archive.state.busy {
            return;
        }
        if let Some(revision) = self.cloud_archive.state.select(id) {
            self.cloud_archive.view = None;
            let req = self.call(READ, json!({"workspaceId":id}));
            self.cloud_archive.read = Some((req, revision));
            cx.notify();
        }
    }
    fn unarchive_cloud(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.cloud_archive.action.is_some()
            || !self
                .cloud_archive
                .state
                .rows
                .iter()
                .any(|r| r.workspace_id == id && r.state == "archived")
        {
            return;
        }
        let command = self
            .cloud_archive
            .commands
            .entry(id.into())
            .or_insert_with(|| uuid::Uuid::new_v4().to_string())
            .clone();
        self.cloud_archive.state.error = None;
        self.cloud_archive.state.busy = true;
        let req = self.call(UNARCHIVE, json!({"workspaceId":id,"commandId":command}));
        self.cloud_archive.action = Some((req, id.into(), false));
        cx.notify();
    }
    pub(super) fn cloud_archive_list(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let query = self.archive_search.read(cx).value(cx).trim().to_lowercase();
        let rows: Vec<_> = self
            .cloud_archive
            .state
            .rows
            .iter()
            .filter(|w| w.archive_visible() && w.label().to_lowercase().contains(&query))
            .cloned()
            .collect();
        s::frame()
            .child(s::header(
                "Cloud sessions",
                "Saved conversations remain available while compute is paused.",
            ))
            .child(
                s::card().children(rows.iter().enumerate().map(|(index, w)| {
                    let open = w.workspace_id.clone();
                    let restore = open.clone();
                    let delete = open.clone();
                    let deleting = w.deleting();
                    let deleted = w.state == "deleted";
                    let cleanup_failed = w.cleanup_failed();
                    let cleanup_id = open.clone();
                    let retry = w.delete_failed();
                    s::setting_row(index == 0)
                        .child(
                            div()
                                .id(("cloud-archive-open", index))
                                .role(Role::Button)
                                .aria_label(format!("Open {}", w.label()))
                                .flex_1()
                                .min_w_0()
                                .cursor_pointer()
                                .child(s::details(
                                    w.label().to_owned(),
                                    (if let Some(label) = w.cleanup_label() {
                                        label
                                    } else if deleting {
                                        w.deletion_label()
                                    } else if w.state == "archiving" {
                                        "Archiving…"
                                    } else {
                                        "Cloud · Archived"
                                    }).to_owned(),
                                ))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if !deleting && !deleted {
                                        this.open_cloud_archive(&open, cx)
                                    }
                                })),
                        )
                        .when(!deleting && !deleted,|view|view.child(
                            s::button(("cloud-archive-restore", index), "Unarchive")
                                .opacity(
                                    if self.cloud_archive.state.busy || w.state != "archived" {
                                        0.45
                                    } else {
                                        1.
                                    },
                                )
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.unarchive_cloud(&restore, cx)
                                })),
                        ))
                        .when(!deleted, |view| view.child(
                            s::button(("cloud-delete-row", index), if deleting {
                                if retry { "Retry delete" } else { "Deleting…" }
                            } else {
                                "Delete…"
                            }).on_click(cx.listener(move |this, _, window, cx| {
                                if this.cloud_archive.action.is_some() {
                                    return;
                                }
                                if retry {
                                    this.delete_cloud(&delete, cx)
                                } else if !deleting {
                                    let focus = [cx.focus_handle(), cx.focus_handle(), cx.focus_handle()];
                                    window.focus(&focus[1], cx);
                                    this.cloud_archive.confirm_focus = Some(focus);
                                    this.cloud_archive.confirm_delete = Some(delete.clone());
                                    cx.notify();
                                }
                            })),
                        ))
                        .when(cleanup_failed, |view| view.child(
                            s::button(("cloud-mirror-cleanup-row", index), "Retry cleanup")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.mutate_cloud(&cleanup_id, CLEANUP_MIRROR, cx)
                                }))
                        ))
                })),
            )
            .when(rows.is_empty(), |view| {
                view.child(
                    s::description_text(if self.cloud_archive.list.is_some() {
                        "Loading cloud sessions…"
                    } else if self.cloud_archive.owner.is_none() {
                        "Sign in to view archived cloud sessions."
                    } else {
                        "No archived cloud sessions."
                    })
                    .p_3(),
                )
            })
            .when(self.cloud_archive.state.loading, |v| {
                v.child(s::description_text("Opening saved history…").p_3())
            })
            .when_some(self.cloud_archive.state.error.clone(), |v, error| {
                v.child(s::description_text(error).p_3())
            })
            .child(
                s::button("cloud-archive-refresh", "Refresh").on_click(cx.listener(
                    |this, _, _, cx| {
                        this.cloud_archive.loaded = false;
                        this.cloud_archive.state.error = None;
                        this.load_cloud_archive();
                        cx.notify();
                    },
                )),
            )
            .when_some(self.cloud_archive.confirm_delete.clone(), |view, id| {
                let focus = self.cloud_archive.confirm_focus.as_ref().unwrap();
                let card = s::card()
                    .id("cloud-delete-dialog")
                    .w(px(420.))
                    .p_5()
                    .role(Role::Dialog)
                    .aria_label("Delete cloud thread?")
                    .child(div().text_size(px(18.)).child("Delete cloud thread?"))
                    .child(s::description_text(
                        "This permanently removes all conversations, cloud files and recovery backups for this thread. This cannot be undone."
                    ).mt_3())
                    .child(
                        row().mt_4().gap_3().justify_end()
                            .child(s::button("cloud-delete-cancel", "Cancel")
                                .track_focus(&focus[1])
                                .tab_index(0)
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.cloud_archive.confirm_delete = None;
                                    window.focus(&this.settings_nav_focus, cx);
                                    cx.notify();
                                })))
                            .child(s::button("cloud-delete-confirm", "Delete permanently")
                                .track_focus(&focus[2])
                                .tab_index(0)
                                .text_color(rgb(0xec9292))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    window.focus(&this.settings_nav_focus, cx);
                                    this.delete_cloud(&id, cx)
                                })))
                    );
                let overlay = div()
                    .w(window.viewport_size().width)
                    .h(window.viewport_size().height)
                    .occlude()
                    .bg(rgba(0x00000099))
                    .flex().items_center().justify_center()
                    .track_focus(&focus[0])
                    .capture_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                        match event.keystroke.key.as_str() {
                            "escape" => {
                                this.cloud_archive.confirm_delete = None;
                                window.focus(&this.settings_nav_focus, cx);
                                cx.notify();
                            }
                            "tab" => if let Some(f) = &this.cloud_archive.confirm_focus {
                                let next = if f[1].is_focused(window) { 2 } else { 1 };
                                window.focus(&f[next], cx);
                            },
                            "enter" | "space" => {
                                let delete = this.cloud_archive.confirm_focus.as_ref()
                                    .is_some_and(|f| f[2].is_focused(window));
                                let id = this.cloud_archive.confirm_delete.clone();
                                window.focus(&this.settings_nav_focus, cx);
                                if delete {
                                    if let Some(id) = id { this.delete_cloud(&id, cx) }
                                } else {
                                    this.cloud_archive.confirm_delete = None;
                                    cx.notify();
                                }
                            }
                            _ => {}
                        }
                        cx.stop_propagation();
                    }))
                    .child(card);
                view.child(gpui::deferred(
                    gpui::anchored().position(gpui::point(px(0.), px(0.))).child(overlay)
                ).priority(2))
            })
            .into_any_element()
    }

    fn archive_card_action(
        &mut self,
        action: crate::archive_cards::Action,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use crate::archive_cards::Action;
        if let Action::Page(page) = action {
            self.archive_page = page;
            self.settings_scroll.set_offset(point(px(0.), px(0.)));
            cx.notify();
            return;
        }
        let id = match &action {
            Action::Open(id) | Action::Restore(id) | Action::Menu(id) => id,
            Action::Page(_) => unreachable!(),
        };
        let Some(chat) = self
            .chats
            .iter()
            .find(|chat| {
                chat["id"] == *id && chat["archived"] == true && chat["threadId"].is_null()
            })
            .cloned()
        else {
            return;
        };
        match action {
            Action::Open(_) => {
                self.select(chat, cx);
            }
            Action::Restore(_) => self.change_thread(
                chat,
                crate::thread_actions::ThreadChange::Archive(false),
                cx,
            ),
            Action::Menu(_) => self.open_thread_actions(chat, window, cx),
            Action::Page(_) => {}
        }
    }
    pub(super) fn archive_list(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        use crate::archive_cards as cards;
        let query = self.archive_search.read(cx).value(cx).trim().to_lowercase();
        if self.archive_query != query {
            self.archive_query = query.clone();
            self.archive_error = None;
            self.archive_page = 0;
            self.settings_scroll.set_offset(point(px(0.), px(0.)));
        }
        let mut entries = self
            .chats
            .iter()
            .filter(|chat| chat["archived"] == true && chat["threadId"].is_null())
            .filter_map(|value| serde_json::from_value::<zuse_protocol::Chat>(value.clone()).ok())
            .map(|chat| {
                let project = self
                    .spaces
                    .iter()
                    .find(|space| Some(space.id.as_str()) == chat.space_id.as_deref())
                    .map(|space| space.display_name().to_owned())
                    .or_else(|| {
                        chat.cwd
                            .as_deref()
                            .and_then(|p| {
                                p.trim_end_matches(['/', '\\']).rsplit(['/', '\\']).next()
                            })
                            .map(str::to_owned)
                    })
                    .filter(|name| !name.is_empty())
                    .unwrap_or_else(|| "Unassigned project".into());
                let computer = self
                    .archive_devices
                    .iter()
                    .find(|device| device.id == chat.device_id)
                    .map(|device| device.name.clone());
                cards::Entry {
                    chat,
                    project,
                    computer,
                }
            })
            .filter(|entry| entry.matches(&query))
            .collect::<Vec<_>>();
        entries.sort_by(|a, b| {
            b.date()
                .cmp(&a.date())
                .then_with(|| a.chat.id.cmp(&b.chat.id))
        });
        let count = entries.len();
        let current = cards::page(count, self.archive_page);
        if current != self.archive_page {
            self.archive_page = current;
            self.settings_scroll.set_offset(point(px(0.), px(0.)));
        }
        let busy = self.thread_action.is_some();
        let restoring = self.restoring_archived_chat().map(str::to_owned);
        let items = entries
            .into_iter()
            .skip(self.archive_page * cards::PAGE_SIZE)
            .take(cards::PAGE_SIZE)
            .enumerate()
            .map(|(index, entry)| {
                let pending = restoring.as_deref() == Some(entry.chat.id.as_str());
                cards::card(
                    entry,
                    index,
                    cards::State {
                        preview: true,
                        menu: true,
                        restoring: pending,
                        blocked: busy || !self.connected,
                    },
                    Self::archive_card_action,
                    cx,
                )
            })
            .collect::<Vec<_>>();
        let mut saved = column().gap(px(8.)).children(items);
        if count == 0 {
            saved = saved.child(
                div()
                    .py(px(32.))
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(8.))
                    .text_color(crate::palette::rgb(MUTED))
                    .child(icon("project-archive").size(px(28.)))
                    .child(if query.is_empty() {
                        "No saved chats"
                    } else {
                        "No matching saved chats"
                    })
                    .child(s::description_text(if query.is_empty() {
                        "Archived chats will appear here."
                    } else {
                        "Try another title, project, branch or computer."
                    })),
            );
        }
        let pager = cards::pagination(count, self.archive_page, Self::archive_card_action, cx);
        column()
            .gap(px(16.))
            .child(
                s::frame()
                    .child(s::header(
                        "Saved chats",
                        format!(
                            "{} matching conversations across your projects and computers.",
                            count
                        ),
                    ))
                    .child(
                        row()
                            .w_full()
                            .max_w(px(576.))
                            .mx(px(10.))
                            .mb(px(8.))
                            .gap(px(8.))
                            .h(px(32.))
                            .px(px(10.))
                            .rounded(px(7.))
                            .border_1()
                            .border_color(crate::palette::rgb(BORDER))
                            .bg(s::card_bg())
                            .child(
                                icon("magnifer")
                                    .size(px(16.))
                                    .text_color(crate::palette::rgb(MUTED)),
                            )
                            .child(self.archive_search.clone()),
                    )
                    .child(saved)
                    .children(pager),
            )
            .child(self.cloud_archive_list(window, cx))
            .into_any_element()
    }
    pub(super) fn cloud_archive_history(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let head = self.cloud_archive.state.head.as_ref().unwrap().clone();
        if self.cloud_archive.view.is_none() {
            let (requests, _discard) = futures::channel::mpsc::unbounded();
            let (_events, events) = futures::channel::mpsc::unbounded();
            let child = cx.new(|cx| {
                let mut view = Self::new(Transport { requests, events }, window, cx);
                view.install_cached_history(&head);
                view
            });
            self.cloud_archive.view = Some(child);
            window.focus(&self.settings_nav_focus, cx);
        }
        let id = head.workspace_id.clone();
        column()
            .flex_1()
            .min_w_0()
            .h_full()
            .min_h_0()
            .child(
                row()
                    .h(px(40.))
                    .flex_none()
                    .px_4()
                    .gap_3()
                    .border_b_1()
                    .border_color(rgb(BORDER))
                    .child(
                        s::button("cloud-history-back", "Back").on_click(cx.listener(
                            |this, _, _, cx| {
                                this.cloud_archive.state.back();
                                this.cloud_archive.view = None;
                                cx.notify();
                            },
                        )),
                    )
                    .child(
                        div().flex_1().child(
                            head.chat
                                .title
                                .clone()
                                .unwrap_or_else(|| "Cloud conversation".into()),
                        ),
                    )
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(rgb(MUTED))
                            .child("Cloud"),
                    ),
            )
            .child(self.cloud_archive.view.clone().unwrap())
            .child(
                row()
                    .flex_none()
                    .px_4()
                    .py_2()
                    .gap_3()
                    .border_t_1()
                    .border_color(rgb(BORDER))
                    .child(
                        icon("archive-minimalistic")
                            .size(px(16.))
                            .text_color(rgb(MUTED)),
                    )
                    .child(
                        column()
                            .flex_1()
                            .text_size(px(12.))
                            .text_color(rgb(MUTED))
                            .child("This chat is archived.")
                            .when_some(self.cloud_archive.state.error.clone(), |v, error| {
                                v.child(error)
                            }),
                    )
                    .child(
                        s::button(
                            "cloud-history-unarchive",
                            if self.cloud_archive.state.busy {
                                "Unarchiving…"
                            } else {
                                "Unarchive"
                            },
                        )
                        .on_click(cx.listener(move |this, _, _, cx| this.unarchive_cloud(&id, cx))),
                    ),
            )
            .into_any_element()
    }
}
