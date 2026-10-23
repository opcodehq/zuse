//! Settings → Archived (feature-inventory §1.5): archived chats across
//! devices, with Unarchive (Mutate setChatArchived false) and a confirmed
//! Delete all (Mutate deleteChat per archived chat).

use gpui::{
    AnyElement, Context, Entity, SharedString, Subscription, Task, Window, div, prelude::*, px,
};

use zuse_protocol::Chat;
use zuse_rpc::methods;

use crate::popover;
use crate::settings::widgets;
use crate::state::AppState;
use crate::theme::Theme;

/// Archived rows in sidebar (recency) order. Pure. Side chats stay listed:
/// once archived and closed, this is the only place to restore them.
pub fn archived_chats(chats: &[Chat]) -> Vec<&Chat> {
    chats
        .iter()
        .filter(|c| c.archived && c.thread_id.is_none())
        .collect()
}

const ARCHIVE_PAGE_SIZE: usize = 40;

fn archive_page(total: usize, requested: usize) -> usize {
    requested.min(total.saturating_sub(1) / ARCHIVE_PAGE_SIZE)
}

pub struct ArchivedPage {
    state: Entity<AppState>,
    scroll: widgets::PageScroll,
    error: Option<SharedString>,
    /// Chat with an in-flight unarchive (button shows working state).
    busy: Option<String>,
    /// Archived ids captured when Delete all was clicked. The dialog deletes
    /// exactly these, never a chat archived while it was open.
    delete_all_confirm: Option<Vec<String>>,
    /// In-flight Delete all; held so repeat clicks can't start a second run.
    delete_all_task: Option<Task<()>>,
    page: usize,
    task: Option<Task<()>>,
    _observe: Subscription,
    cloud: zuse_model::cloud::ArchivedCloud,
    cloud_loaded: bool,
    cloud_task: Option<Task<()>>,
    cloud_poll_task: Option<Task<()>>,
    cloud_delete_confirm: Option<String>,
    cloud_history: Option<Entity<crate::cloud_history::CloudHistory>>,
    cloud_history_sub: Option<Subscription>,
    cloud_owner: Option<String>,
    cloud_commands: std::collections::HashMap<String, String>,
}

impl ArchivedPage {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let owner = state.read(cx).auth_user().map(|u| u.id.clone());
        let observe = cx.observe(&state, |page: &mut Self, state, cx| {
            let owner = state.read(cx).auth_user().map(|u| u.id.clone());
            if owner != page.cloud_owner {
                page.cloud_owner = owner;
                page.cloud.clear();
                page.cloud_loaded = false;
                page.cloud_task = None;
                page.cloud_poll_task = None;
                page.cloud_delete_confirm = None;
                page.cloud_history = None;
                page.cloud_history_sub = None;
                page.cloud_commands.clear();
            }
            cx.notify();
        });
        Self {
            state,
            scroll: widgets::PageScroll::default(),
            error: None,
            busy: None,
            delete_all_confirm: None,
            delete_all_task: None,
            page: 0,
            task: None,
            _observe: observe,
            cloud: Default::default(),
            cloud_loaded: false,
            cloud_task: None,
            cloud_poll_task: None,
            cloud_delete_confirm: None,
            cloud_history: None,
            cloud_history_sub: None,
            cloud_owner: owner,
            cloud_commands: Default::default(),
        }
    }

    fn load_cloud(&mut self, cx: &mut Context<Self>) {
        if self.cloud_loaded || self.cloud.loading || self.cloud_owner.is_none() {
            return;
        }
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        self.cloud.loading = true;
        let owner = self.cloud_owner.clone();
        self.cloud_poll_task = Some(cx.spawn(async move |this, cx| {
            loop {
                let result = engine
                    .client()
                    .call(zuse_engine::cloud::LIST, serde_json::json!({}))
                    .await;
                let again = this
                    .update(cx, |page, cx| {
                        if page.cloud_owner != owner {
                            return false;
                        }
                        page.cloud.loading = false;
                        page.cloud_loaded = true;
                        match result {
                            Ok(value) => {
                                if let (Some(account), Ok(rows)) = (
                                    value["accountId"].as_str(),
                                    serde_json::from_value(value["workspaces"].clone()),
                                ) {
                                    page.cloud.list(account.into(), rows);
                                }
                            }
                            Err(zuse_rpc::RpcError::Failed(error)) => {
                                page.cloud.error = Some(error);
                                cx.notify();
                                return false;
                            }
                            Err(_) => {
                                page.cloud.error = Some(
                                    "Cloud sessions are unavailable. Refresh to try again.".into(),
                                );
                                cx.notify();
                                return false;
                            }
                        }
                        cx.notify();
                        page.cloud.needs_poll()
                    })
                    .unwrap_or(false);
                if !again {
                    break;
                }
                cx.background_executor()
                    .timer(std::time::Duration::from_secs(2))
                    .await;
            }
        }));
    }
    fn open_cloud(&mut self, row: zuse_model::cloud::Workspace, cx: &mut Context<Self>) {
        if self.cloud.loading || self.cloud.busy {
            return;
        }
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        let Some(revision) = self.cloud.select(&row.workspace_id) else {
            return;
        };
        let owner = self.cloud_owner.clone();
        let request = row.workspace_id.clone();
        self.cloud_task = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(
                    zuse_engine::cloud::READ,
                    serde_json::json!({"workspaceId":request}),
                )
                .await;
            let _ = this.update(cx, |page, cx| {
                if page.cloud_owner != owner || page.cloud.revision != revision {
                    return;
                }
                page.cloud.loading = false;
                match result {
                    Ok(value) => {
                        if let (Ok(head), Ok(row)) = (
                            serde_json::from_value(value["head"].clone()),
                            serde_json::from_value(value["workspace"].clone()),
                        ) {
                            if page.cloud.receive(revision, head) {
                                let head = page.cloud.head.clone().unwrap();
                                let view = cx.new(|cx| {
                                    crate::cloud_history::CloudHistory::new(
                                        engine.clone(),
                                        row,
                                        head,
                                        cx,
                                    )
                                });
                                page.cloud_history_sub = Some(cx.subscribe(
                                    &view,
                                    |page, _, event: &crate::cloud_history::Event, cx| {
                                        match event {
                                            crate::cloud_history::Event::Back => page.cloud.back(),
                                            crate::cloud_history::Event::Restored(id) => {
                                                page.cloud.restored(id)
                                            }
                                        }
                                        page.cloud_history = None;
                                        page.cloud_history_sub = None;
                                        cx.notify();
                                    },
                                ));
                                page.cloud_history = Some(view);
                            }
                        }
                    }
                    Err(zuse_rpc::RpcError::Failed(error)) => page.cloud.error = Some(error),
                    Err(_) => {
                        page.cloud.error =
                            Some("Saved cloud history is unavailable. Try again.".into())
                    }
                }
                cx.notify();
            });
        }));
        cx.notify();
    }
    fn unarchive_cloud(&mut self, id: String, cx: &mut Context<Self>) {
        if self.cloud.busy {
            return;
        }
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        let command = self
            .cloud_commands
            .entry(id.clone())
            .or_insert_with(|| uuid::Uuid::new_v4().to_string())
            .clone();
        self.cloud.busy = true;
        self.cloud.error = None;
        let owner = self.cloud_owner.clone();
        self.cloud_task = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(
                    zuse_engine::cloud::UNARCHIVE,
                    serde_json::json!({"workspaceId":id,"commandId":command}),
                )
                .await;
            let _ = this.update(cx, |page, cx| {
                if page.cloud_owner != owner {
                    return;
                }
                page.cloud.busy = false;
                match result {
                    Ok(_) => {
                        page.cloud.restored(&id);
                        page.cloud_commands.remove(&id);
                    }
                    Err(zuse_rpc::RpcError::Failed(error)) => page.cloud.error = Some(error),
                    Err(_) => {
                        page.cloud.error = Some("Unarchive could not finish. Try again.".into())
                    }
                };
                cx.notify();
            });
        }));
        cx.notify();
    }
    fn delete_cloud(&mut self, id: String, cx: &mut Context<Self>) {
        self.mutate_cloud(id, zuse_engine::cloud::DELETE, cx)
    }
    fn mutate_cloud(&mut self, id: String, method: &'static str, cx: &mut Context<Self>) {
        if self.cloud.busy {
            return;
        }
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        self.cloud_delete_confirm = None;
        self.cloud.busy = true;
        self.cloud.error = None;
        let command = self
            .cloud_commands
            .entry(format!("{method}:{id}"))
            .or_insert_with(|| uuid::Uuid::new_v4().to_string())
            .clone();
        let owner = self.cloud_owner.clone();
        self.cloud_task = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(
                    method,
                    serde_json::json!({"workspaceId":id,"commandId":command}),
                )
                .await;
            let _ = this.update(cx, |page, cx| {
                if page.cloud_owner != owner {
                    return;
                }
                page.cloud.busy = false;
                match result {
                    Ok(value) => {
                        if let Ok(row) = serde_json::from_value(value["workspace"].clone()) {
                            page.cloud.update_workspace(row);
                            page.cloud_history = None;
                            page.cloud_history_sub = None;
                        }
                    }
                    Err(zuse_rpc::RpcError::Failed(error)) => page.cloud.error = Some(error),
                    Err(_) => {
                        page.cloud.error =
                            Some("Cloud request interrupted. Refresh to check its progress.".into())
                    }
                }
                page.cloud_loaded = false;
                page.load_cloud(cx);
                cx.notify();
            });
        }));
        cx.notify();
    }
    fn render_cloud_delete_dialog(
        &mut self,
        viewport: gpui::Size<gpui::Pixels>,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let id = self.cloud_delete_confirm.clone()?;
        let theme = Theme::of(cx).for_popup();
        let card = popover::dialog_card(&theme)
            .child(popover::dialog_title(&theme, "Delete cloud thread?"))
            .child(div().mt(px(6.)).child(popover::dialog_body(
                &theme,
                "This permanently removes all conversations, cloud files and recovery backups for this thread. This cannot be undone."
            )))
            .child(
                div().mt(px(16.)).flex().justify_end().gap(px(8.))
                    .child(popover::btn_ghost(&theme, "Cancel", "cloud-delete-cancel")
                        .id("cloud-delete-cancel")
                        .on_click(cx.listener(|page, _, _, cx| {
                            page.cloud_delete_confirm = None;
                            cx.notify();
                        })))
                    .child(popover::btn_danger(&theme, "Delete permanently")
                        .id("cloud-delete-confirm")
                        .on_click(cx.listener(move |page, _, _, cx| {
                            page.delete_cloud(id.clone(), cx)
                        })))
            )
            .into_any_element();
        Some(popover::modal("cloud-delete-dialog", viewport, card))
    }
    fn cloud_section(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let rows: Vec<_> = self
            .cloud
            .rows
            .iter()
            .filter(|r| r.archive_visible())
            .cloned()
            .collect();
        if self.cloud_owner.is_none() {
            return None;
        }
        let empty = rows.is_empty() && !self.cloud.loading && self.cloud.error.is_none();
        let theme = Theme::of(cx).for_settings_surface();
        let busy = self.cloud.busy || self.cloud.loading;
        Some(
            div()
                .mt(px(24.))
                .child(widgets::group_with_action(
                    &theme,
                    "Cloud sessions",
                    "",
                    Some(
                        widgets::ghost_action(&theme)
                            .id("cloud-archive-refresh")
                            .role(gpui::Role::Button)
                            .aria_label("Refresh cloud sessions")
                            .child("Refresh")
                            .on_click(cx.listener(|page, _, _, cx| {
                                if !page.cloud.loading && !page.cloud.busy {
                                    page.cloud_loaded = false;
                                    page.cloud.error = None;
                                    page.load_cloud(cx);
                                    cx.notify();
                                }
                            }))
                            .into_any_element(),
                    ),
                ))
                .child(
                    widgets::section_card(&theme)
                        .when(empty, |view| {
                            view.child(
                                div()
                                    .p(px(16.))
                                    .text_size(px(12.))
                                    .text_color(theme.text_muted)
                                    .child("No archived cloud sessions."),
                            )
                        })
                        .when(self.cloud.loading, |view| {
                            view.child(
                                div()
                                    .p(px(16.))
                                    .text_size(px(12.))
                                    .text_color(theme.text_muted)
                                    .child("Loading cloud sessions…"),
                            )
                        })
                        .when_some(self.cloud.error.clone(), |view, error| {
                            view.child(
                                div()
                                    .p(px(16.))
                                    .text_size(px(12.))
                                    .text_color(theme.text_muted)
                                    .child(error),
                            )
                        })
                        .children(rows.into_iter().enumerate().map(|(index, row)| {
                            let open = row.clone();
                            let id = row.workspace_id.clone();
                            let archived = row.state == "archived";
                            let deleted = row.state == "deleted";
                            let cleanup_failed = row.cleanup_failed();
                            let cleanup_id = id.clone();
                            let delete = id.clone();
                            let deleting = row.deleting();
                            let retry = row.delete_failed();
                            widgets::card_row(&theme, index == 0)
                                .child(
                                    crate::icons::icon(crate::icons::ARCHIVE_MINIMALISTIC)
                                        .size(px(16.))
                                        .text_color(theme.text_muted),
                                )
                                .child(
                                    div()
                                        .id(("archived-cloud-open", index))
                                        .role(gpui::Role::Button)
                                        .aria_label("Open archived cloud thread")
                                        .tab_index(0)
                                        .flex_1()
                                        .min_w_0()
                                        .cursor_pointer()
                                        .flex()
                                        .flex_col()
                                        .gap(px(3.))
                                        .text_size(px(13.))
                                        .child(row.label().to_owned())
                                        .child(
                                            div()
                                                .text_size(px(11.))
                                                .text_color(theme.text_muted)
                                                .child(
                                                    (if let Some(label) = row.cleanup_label() {
                                                        label
                                                    } else if deleting {
                                                        row.deletion_label()
                                                    } else {
                                                        "Cloud"
                                                    })
                                                    .to_owned(),
                                                ),
                                        )
                                        .on_click(cx.listener(move |page, _, _, cx| {
                                            if !deleting && !deleted {
                                                page.open_cloud(open.clone(), cx)
                                            }
                                        })),
                                )
                                .when(!deleting && !deleted, |view| {
                                    view.child(
                                        widgets::ghost_action(&theme)
                                            .id(("cloud-restore-row", index))
                                            .role(gpui::Role::Button)
                                            .aria_label("Unarchive cloud thread")
                                            .tab_index(0)
                                            .opacity(if busy || !archived { 0.45 } else { 1. })
                                            .child(if archived {
                                                "Unarchive"
                                            } else {
                                                "Archiving…"
                                            })
                                            .on_click(cx.listener(move |page, _, _, cx| {
                                                if archived {
                                                    page.unarchive_cloud(id.clone(), cx);
                                                }
                                            })),
                                    )
                                })
                                .when(!deleted, |view| {
                                    view.child(
                                        widgets::ghost_action(&theme)
                                            .id(("cloud-delete-row", index))
                                            .role(gpui::Role::Button)
                                            .aria_label(if retry {
                                                "Retry cloud deletion"
                                            } else {
                                                "Delete cloud thread"
                                            })
                                            .child(if deleting {
                                                if retry { "Retry delete" } else { "Deleting…" }
                                            } else {
                                                "Delete…"
                                            })
                                            .on_click(cx.listener(move |page, _, _, cx| {
                                                if page.cloud.busy {
                                                    return;
                                                }
                                                if retry {
                                                    page.delete_cloud(delete.clone(), cx);
                                                } else if !deleting {
                                                    page.cloud_delete_confirm =
                                                        Some(delete.clone());
                                                    cx.notify();
                                                }
                                            })),
                                    )
                                })
                                .when(cleanup_failed, |view| {
                                    view.child(
                                        widgets::ghost_action(&theme)
                                            .id(("cloud-mirror-cleanup-row", index))
                                            .role(gpui::Role::Button)
                                            .aria_label("Retry local mirror cleanup")
                                            .child("Retry cleanup")
                                            .on_click(cx.listener(move |page, _, _, cx| {
                                                page.mutate_cloud(
                                                    cleanup_id.clone(),
                                                    zuse_engine::cloud::CLEANUP_MIRROR,
                                                    cx,
                                                )
                                            })),
                                    )
                                })
                        })),
                )
                .into_any_element(),
        )
    }
    fn unarchive(&mut self, chat_id: String, cx: &mut Context<Self>) {
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        self.busy = Some(chat_id.clone());
        self.error = None;
        let params = serde_json::json!({
            "op": "setChatArchived",
            "chatId": chat_id,
            "archived": false,
        });
        self.task = Some(cx.spawn(async move |this, cx| {
            let result = engine.client().call(methods::MUTATE, params).await;
            this.update(cx, |page, cx| {
                page.busy = None;
                if let Err(err) = result {
                    page.error = Some(format!("Unarchive failed: {err}").into());
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    fn request_delete_all(&mut self, cx: &mut Context<Self>) {
        if self.delete_all_task.is_some() {
            return;
        }
        let ids: Vec<String> = archived_chats(&self.state.read(cx).chats)
            .into_iter()
            .map(|chat| chat.id.clone())
            .collect();
        if !ids.is_empty() {
            self.delete_all_confirm = Some(ids);
            cx.notify();
        }
    }

    /// Escape that reached Settings unclaimed closes the Delete all dialog
    /// first, so it never closes Settings under the dialog (this page
    /// outlives Settings, and the dialog would reopen with it).
    pub(crate) fn dismiss_on_escape(&mut self, cx: &mut Context<Self>) -> bool {
        if self.cloud_delete_confirm.take().is_some() {
            cx.notify();
            return true;
        }
        if self.delete_all_confirm.take().is_none() {
            return false;
        }
        cx.notify();
        true
    }

    fn delete_all(&mut self, cx: &mut Context<Self>) {
        let Some(ids) = self.delete_all_confirm.take() else {
            return;
        };
        if self.delete_all_task.is_some() {
            return;
        }
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            self.error = Some("Engine not connected".into());
            cx.notify();
            return;
        };
        self.error = None;
        // One request per chat, like the sidebar's section archive: the
        // registry tombstones each row, so it leaves this list and the
        // sidebar's Archived shelf as soon as its delete lands.
        self.delete_all_task = Some(cx.spawn(async move |this, cx| {
            let mut failed = 0;
            for chat_id in ids {
                // Unarchived since the confirmation (here, or on another
                // device): no longer part of what the user agreed to delete.
                let still_archived = this
                    .update(cx, |page, cx| {
                        page.state
                            .read(cx)
                            .chats
                            .iter()
                            .any(|chat| chat.id == chat_id && chat.archived)
                    })
                    .unwrap_or(false);
                if !still_archived {
                    continue;
                }
                let params = serde_json::json!({ "op": "deleteChat", "chatId": chat_id });
                if engine.client().call(methods::MUTATE, params).await.is_err() {
                    failed += 1;
                }
            }
            this.update(cx, |page, cx| {
                page.delete_all_task = None;
                if failed > 0 {
                    page.error = Some(delete_all_failure(failed).into());
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    fn render_delete_all_dialog(
        &mut self,
        viewport: gpui::Size<gpui::Pixels>,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let count = self.delete_all_confirm.as_ref()?.len();
        let theme = Theme::of(cx).for_popup();
        // Same card as the sidebar's single "Delete session?" confirmation.
        let card = popover::dialog_card(&theme)
            .child(popover::dialog_title(
                &theme,
                "Delete all archived sessions?",
            ))
            .child(
                div()
                    .mt(px(6.0))
                    .child(popover::dialog_body(&theme, delete_all_body(count))),
            )
            .child(
                div()
                    .mt(px(16.0))
                    .flex()
                    .flex_row()
                    .justify_end()
                    .gap(px(8.0))
                    .child(
                        popover::btn_ghost(&theme, "Cancel", "delete-archived-cancel")
                            .id("delete-archived-cancel")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.delete_all_confirm = None;
                                cx.notify();
                            })),
                    )
                    .child(
                        popover::btn_danger(&theme, "Delete all")
                            .id("delete-archived-confirm")
                            .on_click(cx.listener(|this, _, _, cx| this.delete_all(cx))),
                    ),
            )
            .into_any_element();
        Some(popover::modal("delete-archived-dialog", viewport, card))
    }

    fn on_scroll_hovered(&mut self, hovered: &bool, _: &mut Window, cx: &mut Context<Self>) {
        if self.scroll.set_list_hovered(*hovered) {
            cx.notify();
        }
    }
}

impl popover::ScrollRailHost for ArchivedPage {
    fn rail_bar(&mut self) -> &mut popover::MenuScrollbarState {
        self.scroll.rail_bar()
    }

    fn rail_scroll(&self) -> Option<gpui::ScrollHandle> {
        self.scroll.rail_scroll()
    }
}

impl Render for ArchivedPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.load_cloud(cx);
        if let Some(history) = &self.cloud_history {
            return history.clone().into_any_element();
        }
        crate::toast::take_error(&mut self.error, cx);
        let dialog = self
            .render_cloud_delete_dialog(window.viewport_size(), cx)
            .or_else(|| self.render_delete_all_dialog(window.viewport_size(), cx));
        let theme = Theme::of(cx).for_settings_surface();
        let now = chrono::Utc::now();
        let (rows, device_names, count): (
            Vec<Chat>,
            std::collections::HashMap<String, String>,
            usize,
        ) = {
            let state = self.state.read(cx);
            let archived = archived_chats(&state.chats);
            let count = archived.len();
            let page = archive_page(count, self.page);
            if self.page != page {
                self.page = page;
                self.scroll.reset();
            }
            let rows = archived
                .into_iter()
                .skip(self.page * 40)
                .take(40)
                .cloned()
                .collect();
            let names = state
                .devices
                .iter()
                .map(|d| (d.id.clone(), d.name.clone()))
                .collect();
            (rows, names, count)
        };
        let busy = self.busy.clone();
        let page = self.page;
        let items: Vec<AnyElement> = rows
            .into_iter()
            .enumerate()
            .map(|(ix, chat)| {
                let title: SharedString = chat
                    .title
                    .clone()
                    .unwrap_or_else(|| "Untitled session".into())
                    .into();
                // Unknown device → no fragment at all (zuse renders the
                // device span only when the name resolves).
                let device: Option<SharedString> =
                    device_names.get(&chat.device_id).cloned().map(Into::into);
                let time_ago: SharedString = crate::state::format_time_ago(
                    chat.last_message_at.unwrap_or(chat.created_at),
                    now,
                )
                .into();
                let location: Option<SharedString> =
                    crate::state::chat_location(&chat).map(Into::into);
                let is_busy = busy.as_deref() == Some(chat.id.as_str());
                let chat_id = chat.id.clone();
                // zuse settings.archived.tsx row: archive tile, medium title
                // + tabular time, quiet device · location meta, Unarchive.
                widgets::card_row(&theme, ix == 0)
                    .id(("archived-row", ix))
                    .flex_nowrap()
                    .child(
                        div()
                            .flex_none()
                            .size(px(28.0))
                            .rounded(px(7.0))
                            .border_1()
                            .border_color(theme.border)
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(
                                crate::icons::icon(crate::icons::ARCHIVE_MINIMALISTIC)
                                    .size(px(16.0))
                                    .text_color(theme.text_muted.opacity(0.6)),
                            ),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .gap(px(8.0))
                                    .child(
                                        div()
                                            .min_w_0()
                                            .truncate()
                                            .text_size(crate::typography::ui_rems(
                                                widgets::ROW_TITLE_SIZE,
                                            ))
                                            .font_weight(gpui::FontWeight::MEDIUM)
                                            .text_color(theme.text_strong)
                                            .child(title),
                                    )
                                    .child(
                                        div()
                                            .flex_none()
                                            .text_size(crate::typography::text_xs())
                                            .text_color(theme.text_muted)
                                            .child(time_ago),
                                    ),
                            )
                            .child({
                                // device · location, separator at the line's
                                // own tone (zuse: a plain span inheriting
                                // `text-muted-foreground/55`).
                                let mut meta = div()
                                    .mt(px(2.0))
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .gap(px(6.0))
                                    .text_size(crate::typography::text_xs())
                                    .text_color(theme.text_muted);
                                let both = device.is_some() && location.is_some();
                                if let Some(device) = device {
                                    meta = meta.child(device);
                                }
                                if both {
                                    meta = meta.child(SharedString::from("·"));
                                }
                                if let Some(location) = location {
                                    meta = meta.child(div().min_w_0().truncate().child(location));
                                }
                                meta
                            }),
                    )
                    .child(
                        // Keep the action visible without list-wide hover invalidation.
                        div()
                            .id(("unarchive", ix))
                            .flex_none()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(6.0))
                            .h(px(28.0))
                            .px(px(10.0))
                            .rounded(px(7.0))
                            .border_1()
                            .border_color(theme.border)
                            .text_size(crate::typography::text_xs())
                            .text_color(theme.text_muted)
                            .opacity(0.8)
                            .when(is_busy, |el| el.opacity(0.4))
                            .cursor_pointer()
                            .hover(|s| s.bg(theme.surface_raised).text_color(theme.text))
                            .tab_index(0)
                            .role(gpui::Role::Button)
                            .focus_visible(|s| s.border_2().border_color(theme.accent).opacity(1.0))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.unarchive(chat_id.clone(), cx);
                            }))
                            .child(
                                crate::icons::icon(crate::icons::ARCHIVE_UP_MINIMALISTIC)
                                    .size(px(14.0))
                                    .text_color(theme.text_muted),
                            )
                            .child(SharedString::from(if is_busy {
                                "Unarchiving…"
                            } else {
                                "Unarchive"
                            })),
                    )
                    .into_any_element()
            })
            .collect();

        let body: AnyElement = if items.is_empty() {
            // Centered empty state (zuse settings.archived.tsx).
            div()
                .py(px(40.0))
                .flex()
                .flex_col()
                .items_center()
                .text_center()
                .text_color(theme.text_muted)
                .child(
                    // `opacity-40` on top of the inherited muted/50 — an
                    // effectively ~20% glyph (zuse settings.archived.tsx).
                    crate::icons::icon(crate::icons::ARCHIVE_MINIMALISTIC)
                        .size(px(28.0))
                        .text_color(theme.text_muted.opacity(0.2)),
                )
                .child(
                    div()
                        .mt(px(12.0))
                        .text_size(crate::typography::text_sm())
                        .child(SharedString::from("Nothing archived")),
                )
                .child(
                    div()
                        .mt(px(4.0))
                        .text_size(crate::typography::text_xs())
                        .text_color(theme.text_muted)
                        .child(SharedString::from(
                            "Right-click a session in the sidebar to archive it.",
                        )),
                )
                .into_any_element()
        } else {
            div().flex().flex_col().children(items).into_any_element()
        };

        let pagination = (count > ARCHIVE_PAGE_SIZE).then(|| {
            div()
                .px(px(12.0))
                .py(px(8.0))
                .border_t_1()
                .border_color(widgets::row_divider(&theme))
                .flex()
                .items_center()
                .justify_between()
                .text_size(crate::typography::text_xs())
                .text_color(theme.text_muted)
                .child(
                    widgets::ghost_action(&theme)
                        .id("archived-previous")
                        .role(gpui::Role::Button)
                        .aria_label("Previous archived sessions")
                        .tab_index(0)
                        .opacity(if page > 0 { 1.0 } else { 0.4 })
                        .focus_visible(|s| s.border_2().border_color(theme.accent))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.page = this.page.saturating_sub(1);
                            this.scroll.reset();
                            cx.notify();
                        }))
                        .child("Previous"),
                )
                .child(format!(
                    "{}–{} of {}",
                    page * ARCHIVE_PAGE_SIZE + 1,
                    ((page + 1) * ARCHIVE_PAGE_SIZE).min(count),
                    count
                ))
                .child(
                    widgets::ghost_action(&theme)
                        .id("archived-next")
                        .role(gpui::Role::Button)
                        .aria_label("Next archived sessions")
                        .tab_index(0)
                        .opacity(if (page + 1) * ARCHIVE_PAGE_SIZE < count {
                            1.0
                        } else {
                            0.4
                        })
                        .focus_visible(|s| s.border_2().border_color(theme.accent))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.page = archive_page(count, this.page + 1);
                            this.scroll.reset();
                            cx.notify();
                        }))
                        .child("Next"),
                )
        });
        let deleting = self.delete_all_task.is_some();
        let delete_all = (count > 0).then(|| {
            widgets::action_button(&theme, widgets::ActionTone::Quiet)
                .text_color(theme.danger)
                .id("archived-delete-all")
                .flex_none()
                .when(deleting, |el| el.opacity(0.5).cursor_default())
                .tab_index(0)
                .role(gpui::Role::Button)
                .focus_visible(|s| s.border_2().border_color(theme.accent))
                .on_click(cx.listener(|this, _, _, cx| this.request_delete_all(cx)))
                .child(
                    crate::icons::icon(crate::icons::TRASH_BIN_MINIMALISTIC)
                        .size(px(14.0))
                        .text_color(theme.danger),
                )
                .child(SharedString::from(if deleting {
                    "Deleting…"
                } else {
                    "Delete all"
                }))
        });
        let scrollbar = popover::rail(self, "archived-page-scrollbar", &theme, cx);
        div()
            .id("archived-page-host")
            .relative()
            .size_full()
            .on_hover(cx.listener(Self::on_scroll_hovered))
            .child(
                crate::edge_fade::edge_faded(
                    16.0,
                    true,
                    true,
                    div()
                        .id("archived-page")
                        .size_full()
                        .overflow_y_scroll()
                        .track_scroll(&self.scroll.scroll)
                        .child(
                            widgets::page_column()
                                .child(widgets::section_title_with_action(
                                    &theme,
                                    "Archived sessions",
                                    (count > 0).then_some(count),
                                    "Hidden from the sidebar until restored.",
                                    None,
                                ))
                                .child(
                                    widgets::section_stack().child(
                                        widgets::group_with_action(
                                            &theme,
                                            "Sessions",
                                            "",
                                            delete_all.map(IntoElement::into_any_element),
                                        )
                                        .child(
                                            widgets::section_card(&theme)
                                                .mt_0()
                                                .child(body)
                                                .children(pagination),
                                        ),
                                    ),
                                )
                                .children(self.cloud_section(cx)),
                        ),
                )
                .fade_overflow_y(&self.scroll.scroll),
            )
            .children(scrollbar)
            .children(dialog)
            .into_any_element()
    }
}

fn delete_all_body(count: usize) -> String {
    match count {
        1 => "1 archived session will be permanently deleted. This can\u{2019}t be undone.".into(),
        n => format!(
            "{n} archived sessions will be permanently deleted. This can\u{2019}t be undone."
        ),
    }
}

fn delete_all_failure(failed: usize) -> String {
    match failed {
        1 => "Couldn\u{2019}t delete 1 archived session. Try again.".into(),
        n => format!("Couldn\u{2019}t delete {n} archived sessions. Try again."),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn chat(id: &str, archived: bool) -> Chat {
        Chat {
            id: id.into(),
            device_id: "d".into(),
            title: None,
            archived,
            cwd: None,
            branch: None,
            checkout_id: None,
            source_context: None,
            config: None,
            last_message_preview: None,
            last_message_at: None,
            created_at: Utc::now(),
            harness_session_id: None,
            harness_session_cwd: None,
            parent_chat_id: None,
            thread_id: None,
            space_id: None,
            last_seen_at: None,
            room_gen: None,
        }
    }

    #[test]
    fn archive_page_clamps_after_removal_and_handles_exact_boundaries() {
        assert_eq!(archive_page(0, 9), 0);
        assert_eq!(archive_page(40, 1), 0);
        assert_eq!(archive_page(41, 1), 1);
        assert_eq!(archive_page(80, 2), 1);
        assert_eq!(archive_page(81, 2), 2);
    }

    #[test]
    fn only_archived_rows_show() {
        let chats = vec![chat("a", false), chat("b", true), chat("c", true)];
        let rows = archived_chats(&chats);
        let ids: Vec<&str> = rows.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, ["b", "c"]);
    }

    #[gpui::test]
    fn delete_all_deletes_the_confirmed_archived_chats_and_reports_failures(
        cx: &mut gpui::TestAppContext,
    ) {
        use gpui::AppContext;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _guard = runtime.enter();
        let (out, mut requests) = tokio::sync::mpsc::channel(16);
        let (replies, inbound) = tokio::sync::mpsc::channel(16);
        let engine =
            crate::state::EngineHandle::from_test_client(zuse_rpc::RpcClient::new(out, inbound));
        let dir = tempfile::tempdir().unwrap();
        cx.update(|cx| {
            crate::settings::init(Default::default(), dir.path(), cx);
            gpui_base::init(cx);
            cx.set_global(crate::theme::Theme::default());
        });
        let window = cx.add_window(|_, cx| {
            let state = cx.new(|_| {
                let mut state = AppState::new();
                state.chats = vec![
                    chat("live", false),
                    chat("b", true),
                    chat("c", true),
                    chat("restored", true),
                ];
                state.set_test_engine(engine);
                state
            });
            ArchivedPage::new(state, cx)
        });
        window
            .update(cx, |page, _, cx| {
                // Escape closes the dialog before it can close Settings.
                page.request_delete_all(cx);
                assert!(page.dismiss_on_escape(cx));
                assert!(page.delete_all_confirm.is_none());
                assert!(!page.dismiss_on_escape(cx));
                page.request_delete_all(cx);
                assert_eq!(
                    page.delete_all_confirm.as_deref(),
                    Some(&["b".to_string(), "c".to_string(), "restored".to_string()][..])
                );
                // Archived while the dialog was open: not part of what the
                // user confirmed, so it must survive.
                page.state
                    .update(cx, |state, _| state.chats.push(chat("late", true)));
                page.delete_all(cx);
                assert!(page.delete_all_confirm.is_none());
            })
            .unwrap();
        for (index, expected) in ["b", "c"].into_iter().enumerate() {
            cx.run_until_parked();
            let request: serde_json::Value = serde_json::from_str(
                &requests
                    .try_recv()
                    .expect("Each confirmed archived chat must be deleted"),
            )
            .unwrap();
            assert_eq!(request["params"]["op"], "deleteChat");
            assert_eq!(request["params"]["chatId"], expected);
            if index == 0 {
                // Unarchived mid-run (e.g. on another device): spared.
                window
                    .update(cx, |page, _, cx| {
                        page.state.update(cx, |state, _| {
                            state
                                .chats
                                .iter_mut()
                                .filter(|chat| chat.id == "restored")
                                .for_each(|chat| chat.archived = false)
                        })
                    })
                    .unwrap();
            }
            let reply = if index == 0 {
                serde_json::json!({"id":request["id"],"err":"rejected"})
            } else {
                serde_json::json!({"id":request["id"],"ok":{"ok":true}})
            };
            runtime.block_on(async {
                replies.send(reply.to_string()).await.unwrap();
                while replies.capacity() < replies.max_capacity() {
                    tokio::task::yield_now().await;
                }
            });
        }
        cx.run_until_parked();
        assert!(
            requests.try_recv().is_err(),
            "Only the confirmed chats are deleted"
        );
        window
            .update(cx, |page, _, cx| {
                assert!(page.delete_all_task.is_none());
                let notice = page
                    .error
                    .clone()
                    .or_else(|| crate::toast::messages(cx).last().cloned());
                assert_eq!(
                    notice.as_deref(),
                    Some("Couldn\u{2019}t delete 1 archived session. Try again.")
                );
            })
            .unwrap();
    }

    #[test]
    fn delete_all_copy_handles_one_and_many() {
        assert!(delete_all_body(1).starts_with("1 archived session will"));
        assert!(delete_all_body(146).starts_with("146 archived sessions will"));
    }

    #[test]
    fn archived_side_chats_stay_restorable() {
        let mut side = chat("side", true);
        side.parent_chat_id = Some("main".into());
        let chats = vec![chat("main", false), side];
        let ids: Vec<&str> = archived_chats(&chats)
            .iter()
            .map(|c| c.id.as_str())
            .collect();
        assert_eq!(ids, ["side"]);
    }
}
