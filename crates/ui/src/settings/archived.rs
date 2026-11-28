//! Settings → Archived (feature-inventory §1.5): archived chats across
//! devices, with Unarchive (Mutate setChatArchived false) and a confirmed
//! Delete all (Mutate deleteChat per archived chat).

use crate::localization::message as tr;
use gpui::{
    AnyElement, App, Context, Entity, SharedString, Subscription, Task, Window, div, prelude::*, px,
};
use zuse_i18n::messages as msg;

use zuse_protocol::Chat;
use zuse_rpc::methods;

use crate::icons::{self, icon};
use crate::popover;
use crate::settings::widgets;
use crate::state::AppState;
use crate::theme::Theme;
use zuse_viewport::cloud_archive_cards as cloud_cards;

fn cloud_owner(state: &AppState) -> Option<String> {
    if !zuse_viewport::cloud_gate::enabled() {
        return None;
    }
    state
        .cloud_account
        .as_ref()
        .filter(|account| account["orgId"].is_null())?;
    state.cloud_user_id().map(str::to_owned)
}
fn cloud_params(owner: &Option<String>, mut params: serde_json::Value) -> serde_json::Value {
    params["expectedAccountId"] = serde_json::json!(owner);
    params["expectedOrganizationId"] = serde_json::Value::Null;
    params
}
fn cloud_bound(value: &serde_json::Value, owner: &Option<String>) -> bool {
    owner.is_some() && value["accountId"].as_str() == owner.as_deref()
}

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

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum SourceFilter {
    #[default]
    All,
    Saved,
    Cloud,
}

impl SourceFilter {
    fn label(self) -> &'static str {
        match self {
            Self::All => "All chats",
            Self::Saved => "Saved chats",
            Self::Cloud => "Cloud chats",
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
enum ArchiveMenu {
    Source,
    Project,
    Group(String),
}

enum ArchiveRow {
    Saved(zuse_viewport::archive_cards::Entry),
    Cloud(zuse_model::cloud::Workspace),
}

impl ArchiveRow {
    fn date(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        match self {
            Self::Saved(entry) => Some(entry.date()),
            Self::Cloud(row) => row
                .updated_at
                .or(row.created_at)
                .and_then(chrono::DateTime::from_timestamp_millis),
        }
    }
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
    search: Option<Entity<zuse_viewport::input::TextInput>>,
    query: String,
    source: SourceFilter,
    project: Option<String>,
    menu: Option<ArchiveMenu>,
    cloud_details: Option<String>,
    _search: Option<Subscription>,
    task: Option<Task<()>>,
    _observe: Subscription,
    preview: Option<Entity<crate::project_archive::ProjectArchive>>,
    preview_events: Option<Subscription>,
    cloud: zuse_model::cloud::ArchivedCloud,
    cloud_loaded: bool,
    cloud_task: Option<Task<()>>,
    cloud_poll_task: Option<Task<()>>,
    cloud_delete_confirm: Option<String>,
    cloud_delete_focus: Option<[gpui::FocusHandle; 3]>,
    cloud_delete_return: Option<gpui::FocusHandle>,
    cloud_history: Option<Entity<crate::cloud_history::CloudHistory>>,
    cloud_history_sub: Option<Subscription>,
    cloud_owner: Option<String>,
    cloud_epoch: u64,
    cloud_action: Option<cloud_cards::Action>,
    cloud_commands: std::collections::HashMap<String, String>,
    /// Last cloud task list revision this page loaded against.
    cloud_revision: u64,
    /// The task list changed while the page could not reload (an operation
    /// in flight or saved history open); reload once it can.
    cloud_stale: bool,
}

impl ArchivedPage {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let owner = cloud_owner(state.read(cx));
        let cloud_revision = state.read(cx).cloud_sidebar_workspaces_revision();
        let observe = cx.observe(&state, |page: &mut Self, state, cx| {
            // A `workspace` account event refreshed the cloud task list (or a
            // task was archived from the sidebar): reload archived tasks on
            // the next paint instead of polling for them.
            let revision = state.read(cx).cloud_sidebar_workspaces_revision();
            if revision != page.cloud_revision {
                page.cloud_revision = revision;
                page.cloud_stale = true;
            }
            page.apply_cloud_staleness();
            let owner = cloud_owner(state.read(cx));
            if owner != page.cloud_owner {
                page.cloud_owner = owner;
                page.cloud_epoch = page.cloud_epoch.wrapping_add(1);
                page.cloud_action = None;
                page.cloud.clear();
                page.cloud_loaded = false;
                page.cloud_task = None;
                page.cloud_poll_task = None;
                page.cloud_delete_confirm = None;
                page.cloud_delete_focus = None;
                page.cloud_delete_return = None;
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
            search: None,
            query: String::new(),
            source: SourceFilter::default(),
            project: None,
            menu: None,
            cloud_details: None,
            _search: None,
            task: None,
            _observe: observe,
            preview: None,
            preview_events: None,
            cloud: Default::default(),
            cloud_loaded: false,
            cloud_task: None,
            cloud_poll_task: None,
            cloud_delete_confirm: None,
            cloud_delete_focus: None,
            cloud_delete_return: None,
            cloud_history: None,
            cloud_history_sub: None,
            cloud_owner: owner,
            cloud_epoch: 0,
            cloud_action: None,
            cloud_commands: Default::default(),
            cloud_revision,
            cloud_stale: false,
        }
    }

    /// Turn a pending task-list change into a reload once nothing in flight
    /// or on screen depends on the current rows.
    fn apply_cloud_staleness(&mut self) {
        if self.cloud_stale
            && !self.cloud.loading
            && !self.cloud.busy
            && self.cloud_history.is_none()
        {
            self.cloud_stale = false;
            self.cloud_loaded = false;
        }
    }
    fn load_cloud(&mut self, cx: &mut Context<Self>) {
        if self.cloud_loaded
            || self.cloud.loading
            || self.cloud.busy
            || self.cloud_owner.is_none()
            || self.cloud_history.is_some()
        {
            return;
        }
        let Some(engine) = self
            .state
            .read(cx)
            .engine()
            .map(|engine| engine.controller())
        else {
            return;
        };
        self.cloud.loading = true;
        let owner = self.cloud_owner.clone();
        let epoch = self.cloud_epoch;
        self.cloud_poll_task = Some(cx.spawn(async move |this, cx| {
            loop {
                let ready = this
                    .update(cx, |page, cx| {
                        if page.cloud_owner != owner
                            || page.cloud_epoch != epoch
                            || page.cloud.busy
                            || page.cloud_history.is_some()
                        {
                            return false;
                        }
                        page.cloud.loading = true;
                        cx.notify();
                        true
                    })
                    .unwrap_or(false);
                if !ready {
                    break;
                }
                let result = engine
                    .client()
                    .call(
                        zuse_engine::cloud::LIST,
                        cloud_params(&owner, serde_json::json!({})),
                    )
                    .await;
                let again = this
                    .update(cx, |page, cx| {
                        if page.cloud_owner != owner || page.cloud_epoch != epoch {
                            return false;
                        }
                        page.cloud.loading = false;
                        page.cloud_loaded = true;
                        match result {
                            Ok(value) if cloud_bound(&value, &owner) => {
                                if let (Some(account), Ok(rows)) = (
                                    value["accountId"].as_str(),
                                    serde_json::from_value(value["workspaces"].clone()),
                                ) {
                                    page.cloud.list(account.into(), rows);
                                } else {
                                    page.cloud_action = None;
                                    page.cloud.error = Some(
                                        "Cloud archives are unavailable. Refresh to try again."
                                            .into(),
                                    );
                                    cx.notify();
                                    return false;
                                }
                            }
                            Ok(_) => {
                                page.cloud_action = None;
                                page.cloud.error = Some(
                                    "Your account changed. Refresh to load its Cloud archives."
                                        .into(),
                                );
                                cx.notify();
                                return false;
                            }
                            Err(zuse_rpc::RpcError::Failed(error)) => {
                                page.cloud_action = None;
                                page.cloud.error = Some(error);
                                cx.notify();
                                return false;
                            }
                            Err(_) => {
                                page.cloud_action = None;
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
        if self.cloud.loading || self.cloud.busy || self.cloud_owner.is_none() {
            return;
        }
        let Some(engine) = self
            .state
            .read(cx)
            .engine()
            .map(|engine| engine.controller())
        else {
            return;
        };
        self.cloud_poll_task = None;
        let Some(revision) = self.cloud.select(&row.workspace_id) else {
            return;
        };
        let owner = self.cloud_owner.clone();
        let epoch = self.cloud_epoch;
        let request = row.workspace_id.clone();
        self.cloud_action = Some(cloud_cards::Action::Open(request.clone()));
        self.cloud_task = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(
                    zuse_engine::cloud::READ,
                    cloud_params(&owner, serde_json::json!({"workspaceId":request})),
                )
                .await;
            let _ = this.update(cx, |page, cx| {
                if page.cloud_owner != owner || page.cloud_epoch != epoch || page.cloud.revision != revision {
                    return;
                }
                page.cloud.loading = false;
                match result {
                    Ok(value) if cloud_bound(&value, &owner) => {
                        if let (Ok(head), Ok(row)) = (
                            serde_json::from_value(value["head"].clone()),
                            serde_json::from_value::<zuse_model::cloud::Workspace>(value["workspace"].clone()),
                        ) {
                            if row.workspace_id == request && page.cloud.receive(revision, head) {
                                let head = page.cloud.head.clone().unwrap();
                                let view = cx.new(|cx| {
                                    crate::cloud_history::CloudHistory::new(
                                        engine.clone(),
                                        row,
                                        head,
                                        owner.clone().unwrap(),
                                        cx,
                                    )
                                });
                                page.cloud_history_sub = Some(cx.subscribe(
                                    &view,
                                    |page, _, event: &crate::cloud_history::Event, cx| {
                                        match event {
                                            crate::cloud_history::Event::Resume { .. } => {},
                                            crate::cloud_history::Event::Back => {page.cloud.back();page.cloud_loaded=false;},
                                            crate::cloud_history::Event::Restored(id) => {
                                                page.cloud.restored(id);page.cloud_loaded=false;
                                            }
                                            crate::cloud_history::Event::Connect(session) => {
                                                if let (Some(engine), Some(head), Some(owner)) = (
                                                    page.state.read(cx).engine(),
                                                    page.cloud.head.as_ref(),
                                                    page.cloud_owner.as_ref(),
                                                ) {
                                                    crate::open_cloud_window(
                                                        crate::state::CloudTarget {
                                                            controller: engine.controller(),
                                                            subject: owner.clone(),
                                                            organization: None,
                                                            workspace: head.workspace_id.clone(),
                                                            initial_session: session.clone(),
                                                        },
                                                        cx,
                                                    );
                                                }
                                            }
                                        }
                                        page.cloud_history = None;
                                        page.cloud_history_sub = None;
                                        cx.notify();
                                    },
                                ));
                                page.cloud_history = Some(view);
                            } else { page.cloud.error = Some("Saved history does not match this Cloud chat. Refresh to try again.".into()); }
                        } else { page.cloud.error = Some("Saved Cloud history is unavailable. Try again.".into()); }
                    }
                    Ok(_) => page.cloud.error = Some("Your account changed. Refresh to load its Cloud archives.".into()),
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
        if self.cloud.busy
            || self.cloud.loading
            || self.cloud_owner.is_none()
            || !self
                .cloud
                .rows
                .iter()
                .any(|row| row.workspace_id == id && row.state == "archived" && !row.deleting())
        {
            return;
        }
        let Some(engine) = self
            .state
            .read(cx)
            .engine()
            .map(|engine| engine.controller())
        else {
            return;
        };
        let command = self
            .cloud_commands
            .entry(id.clone())
            .or_insert_with(|| uuid::Uuid::new_v4().to_string())
            .clone();
        self.cloud_poll_task = None;
        self.cloud.busy = true;
        self.cloud.error = None;
        self.cloud_action = Some(cloud_cards::Action::Restore(id.clone()));
        let owner = self.cloud_owner.clone();
        let epoch = self.cloud_epoch;
        self.cloud_task = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(
                    zuse_engine::cloud::UNARCHIVE,
                    cloud_params(
                        &owner,
                        serde_json::json!({"workspaceId":id,"commandId":command}),
                    ),
                )
                .await;
            let _ = this.update(cx, |page, cx| {
                if page.cloud_owner != owner || page.cloud_epoch != epoch {
                    return;
                }
                page.cloud.busy = false;
                match result {
                    Ok(value)
                        if cloud_bound(&value, &owner)
                            && value["workspace"]["workspaceId"] == id =>
                    {
                        page.cloud.restored(&id);
                        page.cloud_commands.remove(&id);
                    }
                    Ok(_) => {
                        page.cloud.error = Some(
                            "This restore could not be verified. Refresh to check its state."
                                .into(),
                        )
                    }
                    Err(zuse_rpc::RpcError::Failed(error)) => page.cloud.error = Some(error),
                    Err(_) => {
                        page.cloud.error = Some("Unarchive could not finish. Try again.".into())
                    }
                };
                if page.cloud.needs_poll() {
                    page.cloud_loaded = false;
                    page.load_cloud(cx);
                }
                cx.notify();
            });
        }));
        cx.notify();
    }
    fn delete_cloud(&mut self, id: String, cx: &mut Context<Self>) {
        self.mutate_cloud(id, zuse_engine::cloud::DELETE, cx)
    }
    fn mutate_cloud(&mut self, id: String, method: &'static str, cx: &mut Context<Self>) {
        if self.cloud.busy || self.cloud.loading || self.cloud_owner.is_none() {
            return;
        }
        let Some(engine) = self
            .state
            .read(cx)
            .engine()
            .map(|engine| engine.controller())
        else {
            return;
        };
        self.cloud_poll_task = None;
        self.cloud_delete_confirm = None;
        self.cloud.busy = true;
        self.cloud.error = None;
        self.cloud_action = Some(if method == zuse_engine::cloud::CLEANUP_MIRROR {
            cloud_cards::Action::Cleanup(id.clone())
        } else {
            cloud_cards::Action::Delete(id.clone())
        });
        let command = self
            .cloud_commands
            .entry(format!("{method}:{id}"))
            .or_insert_with(|| uuid::Uuid::new_v4().to_string())
            .clone();
        let owner = self.cloud_owner.clone();
        let epoch = self.cloud_epoch;
        self.cloud_task = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(
                    method,
                    cloud_params(
                        &owner,
                        serde_json::json!({"workspaceId":id,"commandId":command}),
                    ),
                )
                .await;
            let _ = this.update(cx, |page, cx| {
                if page.cloud_owner != owner || page.cloud_epoch != epoch {
                    return;
                }
                page.cloud.busy = false;
                match result {
                    Ok(value)
                        if cloud_bound(&value, &owner)
                            && value["workspace"]["workspaceId"] == id =>
                    {
                        if let Ok(row) = serde_json::from_value(value["workspace"].clone()) {
                            page.cloud.update_workspace(row);
                            page.cloud_history = None;
                            page.cloud_history_sub = None;
                        }
                    }
                    Ok(_) => {
                        page.cloud.error = Some(
                            "This request could not be verified. Refresh to check its state."
                                .into(),
                        )
                    }
                    Err(zuse_rpc::RpcError::Failed(error)) => page.cloud.error = Some(error),
                    Err(_) => {
                        page.cloud.error =
                            Some("Cloud request interrupted. Refresh to check its progress.".into())
                    }
                }
                if page.cloud.error.is_none() {
                    page.cloud_action = None;
                    page.cloud_loaded = false;
                    page.load_cloud(cx);
                }
                cx.notify();
            });
        }));
        cx.notify();
    }
    fn cancel_cloud_delete(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.cloud_delete_confirm = None;
        self.cloud_delete_focus = None;
        if let Some(focus) = self.cloud_delete_return.take() {
            window.focus(&focus, cx);
        }
        cx.notify();
    }
    fn render_cloud_delete_dialog(
        &mut self,
        viewport: gpui::Size<gpui::Pixels>,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let id = self.cloud_delete_confirm.clone()?;
        let focus = self.cloud_delete_focus.clone()?;
        let theme = Theme::of(cx).for_popup();
        let card = popover::dialog_card(&theme)
            .id("cloud-delete-card").role(gpui::Role::Dialog).aria_label(tr(cx, msg::native::delete_cloud_chat))
            .track_focus(&focus[0]).tab_group().tab_stop(false)
            .capture_key_down(cx.listener(|page,event:&gpui::KeyDownEvent,window,cx| {
                match event.keystroke.key.as_str() {
                    "escape" => {page.cancel_cloud_delete(window,cx);cx.stop_propagation();}
                    "tab" => {
                        if event.keystroke.modifiers.shift {window.focus_prev(cx);} else {window.focus_next(cx);}
                        if let Some(focus)=&page.cloud_delete_focus {
                            if !focus[0].contains_focused(window,cx) {window.focus(if event.keystroke.modifiers.shift {&focus[2]} else {&focus[1]},cx);}
                        }
                        cx.stop_propagation();
                    }
                    _=>{}
                }
            }))
            .child(popover::dialog_title(&theme, "Delete Cloud chat?"))
            .child(div().mt(px(6.)).child(popover::dialog_body(
                &theme,
                "This permanently removes all conversations, Cloud files and recovery backups for this chat. This cannot be undone."
            )))
            .child(
                popover::dialog_footer(&theme)
                    .child(popover::btn_ghost(&theme, &tr(cx, msg::settings::organizations_cancel), "cloud-delete-cancel")
                        .id("cloud-delete-cancel").role(gpui::Role::Button).tab_index(0).track_focus(&focus[1])
                        .on_click(cx.listener(|page, _, window, cx| page.cancel_cloud_delete(window,cx))))
                    .child(popover::btn_danger(&theme, "Delete permanently")
                        .id("cloud-delete-confirm").role(gpui::Role::Button).tab_index(0).track_focus(&focus[2])
                        .on_click(cx.listener(move |page, _, window, cx| {
                            page.cancel_cloud_delete(window,cx);
                            page.delete_cloud(id.clone(), cx)
                        })))
            )
            .into_any_element();
        Some(popover::modal("cloud-delete-dialog", viewport, card))
    }
    fn cloud_card_action(
        &mut self,
        action: cloud_cards::Action,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use cloud_cards::Action;
        if self.cloud.busy || self.cloud.loading || self.cloud_owner.is_none() {
            return;
        }
        if let Some(id) = action.target() {
            if !self
                .cloud
                .rows
                .iter()
                .any(|row| row.workspace_id == id && row.archive_visible())
            {
                return;
            }
        }
        match action {
            Action::Refresh => {
                self.cloud_loaded = false;
                self.cloud.error = None;
                self.cloud_action = None;
                self.load_cloud(cx);
            }
            Action::Open(id) => {
                if let Some(row) = self
                    .cloud
                    .rows
                    .iter()
                    .find(|row| row.workspace_id == id && !row.deleting() && row.state != "deleted")
                    .cloned()
                {
                    self.open_cloud(row, cx);
                }
            }
            Action::Restore(id) => self.unarchive_cloud(id, cx),
            Action::Delete(id) => {
                let focus = [
                    cx.focus_handle(),
                    cx.focus_handle().tab_stop(true),
                    cx.focus_handle().tab_stop(true),
                ];
                self.cloud_delete_return = window.focused(cx);
                window.focus(&focus[1], cx);
                self.cloud_delete_focus = Some(focus);
                self.cloud_delete_confirm = Some(id);
            }
            Action::RetryDelete(id) => self.delete_cloud(id, cx),
            Action::Cleanup(id) => self.mutate_cloud(id, zuse_engine::cloud::CLEANUP_MIRROR, cx),
        }
        cx.notify();
    }
    fn unarchive(&mut self, chat_id: String, cx: &mut Context<Self>) {
        if self.busy.is_some() || self.delete_all_task.is_some() {
            return;
        }
        let Some(owner) = self
            .state
            .read(cx)
            .chats
            .iter()
            .find(|chat| chat.id == chat_id && chat.archived)
            .map(|chat| chat.device_id.clone())
        else {
            return;
        };
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        self.state
            .update(cx, |state, _| state.clear_chat_archive_intent(&chat_id));
        self.busy = Some(chat_id.clone());
        self.error = None;
        let params = serde_json::json!({
            "op": "setChatArchived",
            "chatId": chat_id,
            "targetDeviceId": owner,
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

    fn archive_card_action(
        &mut self,
        action: zuse_viewport::archive_cards::Action,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match action {
            zuse_viewport::archive_cards::Action::Open(id) => {
                if self.busy.is_some() || self.delete_all_task.is_some() {
                    return;
                }
                if self.cloud.loading && self.cloud.selected.is_some() && !self.cloud.busy {
                    self.cloud_task = None;
                    self.cloud.back();
                }
                self.error = None;
                let state = self.state.clone();
                let preview = cx.new(|cx| {
                    crate::project_archive::ProjectArchive::global(state, id, window, cx)
                });
                self.preview_events = Some(cx.subscribe(
                    &preview,
                    |this, _, _: &crate::project_archive::Event, cx| {
                        this.preview = None;
                        this.preview_events = None;
                        cx.notify();
                    },
                ));
                self.preview = Some(preview);
                cx.notify();
            }
            zuse_viewport::archive_cards::Action::Restore(id) => self.unarchive(id, cx),
            zuse_viewport::archive_cards::Action::Page(page) => {
                self.page = page;
                self.scroll.reset();
                cx.notify();
            }
            _ => {}
        }
    }
    #[cfg(test)]
    fn request_delete_all(&mut self, cx: &mut Context<Self>) {
        let ids: Vec<String> = archived_chats(&self.state.read(cx).chats)
            .into_iter()
            .map(|chat| chat.id.clone())
            .collect();
        self.request_delete_saved(ids, cx);
    }

    fn request_delete_saved(&mut self, ids: Vec<String>, cx: &mut Context<Self>) {
        if self.delete_all_task.is_none() && self.busy.is_none() && !ids.is_empty() {
            self.delete_all_confirm = Some(ids);
            cx.notify();
        }
    }

    pub(crate) fn on_hidden(&mut self, cx: &mut Context<Self>) {
        self.menu = None;
        if let Some(preview) = self.preview.take() {
            preview.update(cx, |preview, cx| preview.on_hidden(cx));
        }
        self.preview_events = None;
        cx.notify();
    }
    /// Escape that reached Settings unclaimed closes the Delete all dialog
    /// first, so it never closes Settings under the dialog (this page
    /// outlives Settings, and the dialog would reopen with it).
    pub(crate) fn dismiss_on_escape(&mut self, cx: &mut Context<Self>) -> bool {
        if self.menu.take().is_some() {
            cx.notify();
            return true;
        }
        if let Some(preview) = self.preview.take() {
            preview.update(cx, |preview, cx| preview.on_hidden(cx));
            self.preview_events = None;
            cx.notify();
            return true;
        }
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
                let owner = this.update(cx, |page,cx| page.state.read(cx).chats.iter()
                    .find(|chat|chat.id==chat_id&&chat.archived).map(|chat|chat.device_id.clone())).ok().flatten();
                let Some(owner)=owner else{continue;};
                let params = serde_json::json!({ "op": "deleteChat", "chatId": chat_id,"targetDeviceId":owner });
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
                if count == 1 {
                    "Delete archived chat?"
                } else {
                    "Delete archived chats?"
                },
            ))
            .child(
                div()
                    .mt(px(6.0))
                    .child(popover::dialog_body(&theme, delete_all_body(count))),
            )
            .child(
                popover::dialog_footer(&theme)
                    .child(
                        popover::btn_ghost(
                            &theme,
                            &tr(cx, msg::settings::organizations_cancel),
                            "delete-archived-cancel",
                        )
                        .id("delete-archived-cancel")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.delete_all_confirm = None;
                            cx.notify();
                        })),
                    )
                    .child(
                        popover::btn_danger(&theme, "Delete permanently")
                            .id("delete-archived-confirm")
                            .on_click(cx.listener(|this, _, _, cx| this.delete_all(cx))),
                    ),
            )
            .into_any_element();
        Some(popover::modal("delete-archived-dialog", viewport, card))
    }

    fn reset_archive_filters(&mut self, cx: &mut Context<Self>) {
        self.menu = None;
        self.page = 0;
        self.scroll.reset();
        cx.notify();
    }

    fn archive_groups(
        &self,
        cx: &App,
    ) -> std::collections::BTreeMap<(String, String), Vec<ArchiveRow>> {
        let state = self.state.read(cx);
        let cloud_projects = state.cloud_sidebar_projects();
        let labels = state.project_labels(&cloud_projects);
        let cloud_groups = cloud_projects
            .iter()
            .filter_map(|project| {
                zuse_protocol::presentation::cloud_project_group(project, &state.spaces)
            })
            .map(|group| (group.project_id.clone(), group))
            .collect::<std::collections::HashMap<_, _>>();
        let unknown = || ("Unassigned project".to_owned(), "unassigned".to_owned());
        let mut groups = std::collections::BTreeMap::<_, Vec<_>>::new();
        for chat in archived_chats(&state.chats) {
            let (name, key) = state
                .spaces
                .iter()
                .find(|space| Some(space.id.as_str()) == chat.space_id.as_deref())
                .map(|space| {
                    let key = zuse_protocol::view::project_key(space);
                    let name = labels
                        .local
                        .get(&key)
                        .cloned()
                        .unwrap_or_else(|| space.display_name().to_owned());
                    (name, key)
                })
                .or_else(|| {
                    let path = chat.cwd.as_deref()?.trim_end_matches(['/', '\\']);
                    let name = path
                        .rsplit(['/', '\\'])
                        .next()
                        .filter(|name| !name.is_empty())?;
                    Some((name.to_owned(), format!("folder:{}:{path}", chat.device_id)))
                })
                .unwrap_or_else(unknown);
            let computer = state
                .devices
                .iter()
                .find(|device| device.id == chat.device_id)
                .map(|device| device.name.clone());
            groups
                .entry((name.clone(), key))
                .or_default()
                .push(ArchiveRow::Saved(zuse_viewport::archive_cards::Entry {
                    chat: chat.clone(),
                    project: name,
                    computer,
                }));
        }
        if self.cloud_owner.is_some() {
            for row in self.cloud.rows.iter().filter(|row| row.archive_visible()) {
                let (name, key) = row
                    .project_id
                    .as_ref()
                    .map(|id| {
                        if let Some(group) = cloud_groups.get(id) {
                            let name = labels
                                .cloud
                                .get(id)
                                .cloned()
                                .unwrap_or_else(|| group.name.clone());
                            (name, group.key.clone())
                        } else {
                            ("Cloud project".to_owned(), format!("cloud:{id}"))
                        }
                    })
                    .unwrap_or_else(unknown);
                groups
                    .entry((name, key))
                    .or_default()
                    .push(ArchiveRow::Cloud(row.clone()));
            }
        }
        groups
    }

    fn archive_menu(
        &self,
        id: ArchiveMenu,
        rows: Vec<AnyElement>,
        cx: &Context<Self>,
    ) -> AnyElement {
        let theme = Theme::of(cx).for_popup();
        let menu = popover::popover_card(&theme)
            .w(px(218.))
            .child(
                div()
                    .id("archive-menu-options")
                    .flex()
                    .flex_col()
                    .max_h(px(280.))
                    .overflow_y_scroll()
                    .children(rows),
            )
            .on_mouse_down_out(cx.listener(move |this, _, _, cx| {
                if this.menu.as_ref() == Some(&id) {
                    this.menu = None;
                    cx.notify();
                }
            }));
        popover::anchored_menu_below_end("archive-settings-menu", menu.into_any_element(), None)
    }

    fn archive_dropdown(
        &self,
        id: ArchiveMenu,
        glyph: &'static str,
        label: SharedString,
        rows: Vec<AnyElement>,
        theme: &Theme,
        cx: &Context<Self>,
    ) -> AnyElement {
        let open = self.menu.as_ref() == Some(&id);
        let key = if id == ArchiveMenu::Source {
            "archive-source-filter"
        } else {
            "archive-project-filter"
        };
        let open_menu = open.then(|| self.archive_menu(id.clone(), rows, cx));
        div()
            .relative()
            .flex_none()
            .child(
                widgets::action_button(theme, widgets::ActionTone::Outlined)
                    .id(key)
                    .role(gpui::Role::Button)
                    .aria_expanded(open)
                    .tab_index(0)
                    .h(px(28.))
                    .min_w(px(118.))
                    .max_w(px(186.))
                    .px(px(10.))
                    .bg(widgets::block_fill(theme))
                    .child(
                        icon(glyph)
                            .size(px(14.))
                            .flex_none()
                            .text_color(theme.text_muted),
                    )
                    .child(div().flex_1().truncate().child(label))
                    .child(
                        icon(icons::ALT_ARROW_DOWN)
                            .size(px(12.))
                            .flex_none()
                            .text_color(theme.text_muted),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.menu = (!open).then(|| id.clone());
                        cx.notify();
                    })),
            )
            .children(open_menu)
            .into_any_element()
    }

    fn archive_group(
        &self,
        (name, key): (String, String),
        rows: &[ArchiveRow],
        visible: std::ops::Range<usize>,
        blocked: bool,
        theme: &Theme,
        cx: &Context<Self>,
    ) -> AnyElement {
        let saved = rows
            .iter()
            .filter_map(|row| match row {
                ArchiveRow::Saved(entry) => Some(entry.chat.id.clone()),
                ArchiveRow::Cloud(_) => None,
            })
            .collect::<Vec<_>>();
        let count = rows.len();
        let menu_id = ArchiveMenu::Group(key.clone());
        let open = self.menu.as_ref() == Some(&menu_id);
        let group_menu = (!saved.is_empty()).then(|| {
            let count = saved.len();
            let label = format!(
                "Delete {count} saved chat{}",
                if count == 1 { "" } else { "s" }
            );
            let menu = open.then(|| {
                self.archive_menu(
                    menu_id.clone(),
                    vec![
                        popover::menu_row(theme, false, label.clone())
                            .id("archive-delete-group")
                            .text_color(theme.danger)
                            .child(
                                icon(icons::TRASH_BIN_MINIMALISTIC)
                                    .size(px(15.))
                                    .text_color(theme.danger),
                            )
                            .child(label)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.menu = None;
                                this.request_delete_saved(saved.clone(), cx);
                            }))
                            .into_any_element(),
                    ],
                    cx,
                )
            });
            div()
                .relative()
                .child(
                    widgets::action_button(theme, widgets::ActionTone::Quiet)
                        .id("archived-group-more")
                        .role(gpui::Role::Button)
                        .aria_label("Project archive actions")
                        .aria_expanded(open)
                        .tab_index(0)
                        .px(px(7.))
                        .child(
                            icon(icons::MORE_HORIZONTAL)
                                .size(px(16.))
                                .text_color(theme.text_muted),
                        )
                        .when(blocked, |el| el.opacity(0.5).cursor_default())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if !blocked {
                                this.menu = (!open).then(|| menu_id.clone());
                                cx.notify();
                            }
                        })),
                )
                .children(menu)
        });
        let heading = div()
            .flex()
            .items_center()
            .gap(px(7.))
            .child(
                icon(icons::FOLDER)
                    .size(px(14.))
                    .text_color(theme.text_muted),
            )
            .child(widgets::row_title(theme, name).truncate());
        let actions = div()
            .flex()
            .items_center()
            .gap(px(6.))
            .text_size(crate::typography::text_xs())
            .text_color(theme.text_muted)
            .child(format!("{count} chat{}", if count == 1 { "" } else { "s" }))
            .children(group_menu)
            .into_any_element();
        zuse_viewport::settings_style::frame()
            .id(SharedString::from(format!("archived-group-{key}")))
            .bg(theme.settings_frame_bg())
            .child(zuse_viewport::settings_style::header_bar(
                heading,
                Some(actions),
            ))
            .child(
                widgets::section_card(theme)
                    .mt_0()
                    .children(
                        rows[visible]
                            .iter()
                            .enumerate()
                            .map(|(index, row)| match row {
                                ArchiveRow::Saved(entry) => {
                                    self.saved_archive_row(entry, index == 0, blocked, theme, cx)
                                }
                                ArchiveRow::Cloud(row) => {
                                    self.cloud_archive_row(row, index == 0, theme, cx)
                                }
                            }),
                    ),
            )
            .into_any_element()
    }

    fn saved_archive_row(
        &self,
        entry: &zuse_viewport::archive_cards::Entry,
        first: bool,
        blocked: bool,
        theme: &Theme,
        cx: &Context<Self>,
    ) -> AnyElement {
        let id = entry.chat.id.clone();
        let title = entry
            .chat
            .title
            .as_deref()
            .filter(|title| !title.trim().is_empty())
            .unwrap_or("Untitled chat");
        let preview = id.clone();
        let restore = id.clone();
        let delete = id.clone();
        archive_line(theme, first, "saved", &id)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(
                        widgets::row_title(theme, crate::transcript::single_line(title))
                            .id("archive-preview")
                            .truncate()
                            .cursor_pointer()
                            .text_color(theme.text_strong)
                            .hover(|el| el.text_color(theme.accent))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.archive_card_action(
                                    zuse_viewport::archive_cards::Action::Open(preview.clone()),
                                    window,
                                    cx,
                                );
                            })),
                    )
                    .child(archive_date(Some(entry.date()), theme)),
            )
            .child(
                archive_button(
                    "archive-restore",
                    if self.busy.as_deref() == Some(id.as_str()) {
                        "Restoring…"
                    } else {
                        "Unarchive"
                    },
                    false,
                    blocked,
                    theme,
                )
                .on_click(cx.listener(move |this, _, _, cx| this.unarchive(restore.clone(), cx))),
            )
            .child(
                archive_button("archive-delete", "Delete", true, blocked, theme).on_click(
                    cx.listener(move |this, _, _, cx| {
                        if !blocked {
                            this.request_delete_saved(vec![delete.clone()], cx);
                        }
                    }),
                ),
            )
            .into_any_element()
    }

    fn cloud_archive_row(
        &self,
        row: &zuse_model::cloud::Workspace,
        first: bool,
        theme: &Theme,
        cx: &Context<Self>,
    ) -> AnyElement {
        use cloud_cards::Action;
        let id = &row.workspace_id;
        let blocked = self.cloud.loading || self.cloud.busy;
        let pending = self
            .cloud_action
            .as_ref()
            .filter(|action| self.cloud.busy && action.target() == Some(id.as_str()));
        let enabled = !row.deleting() && row.state != "deleted";
        let (status, description, attention) = cloud_cards::status(row);
        let show_details = self.cloud_details.as_deref() == Some(id.as_str());
        let error =
            self.cloud.error.as_deref().filter(|_| {
                self.cloud_action.as_ref().and_then(Action::target) == Some(id.as_str())
            });
        let action = |key, label, danger, blocked, action: Action| {
            archive_button(key, label, danger, blocked, theme).on_click(cx.listener(
                move |this, _, window, cx| {
                    if !blocked {
                        this.cloud_card_action(action.clone(), window, cx);
                    }
                },
            ))
        };
        let open = Action::Open(id.clone());
        let toggle = id.clone();
        let date = row
            .updated_at
            .or(row.created_at)
            .and_then(chrono::DateTime::from_timestamp_millis);
        let line = archive_line(theme, first, "cloud", id)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(
                        widgets::row_title(theme, crate::transcript::single_line(row.label()))
                            .id("cloud-archive-open")
                            .truncate()
                            .text_color(theme.text_strong)
                            .when(enabled && !blocked, |el| {
                                el.cursor_pointer().hover(|el| el.text_color(theme.accent))
                            })
                            .on_click(cx.listener(move |this, _, window, cx| {
                                if enabled {
                                    this.cloud_card_action(open.clone(), window, cx);
                                }
                            })),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(7.))
                            .child(archive_date(date, theme))
                            .child(
                                icon(icons::DITHER_CLOUD)
                                    .size(px(13.))
                                    .text_color(theme.text_muted),
                            )
                            .child(
                                div()
                                    .id("cloud-archive-details")
                                    .role(gpui::Role::Button)
                                    .aria_expanded(show_details)
                                    .tab_index(0)
                                    .cursor_pointer()
                                    .text_size(px(11.))
                                    .text_color(if attention {
                                        theme.danger
                                    } else {
                                        theme.text_muted
                                    })
                                    .hover(|el| el.text_color(theme.text))
                                    .child(status)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.cloud_details =
                                            (!show_details).then(|| toggle.clone());
                                        cx.notify();
                                    })),
                            ),
                    ),
            )
            .when(enabled, |el| {
                el.child(action(
                    "cloud-archive-restore",
                    if matches!(pending, Some(Action::Restore(_))) {
                        "Restoring…"
                    } else {
                        "Unarchive"
                    },
                    false,
                    blocked || row.state != "archived",
                    Action::Restore(id.clone()),
                ))
            })
            .when(row.state != "deleted", |el| {
                let retry = row.delete_failed();
                el.child(action(
                    "cloud-archive-delete",
                    if retry {
                        "Retry deletion"
                    } else if row.deleting() || matches!(pending, Some(Action::Delete(_))) {
                        "Deleting…"
                    } else {
                        "Delete"
                    },
                    true,
                    blocked || (row.deleting() && !retry),
                    if retry {
                        Action::RetryDelete(id.clone())
                    } else {
                        Action::Delete(id.clone())
                    },
                ))
            });
        div()
            .child(line)
            .when(
                show_details || error.is_some() || row.cleanup_failed(),
                |el| {
                    el.child(
                        div()
                            .px(px(12.))
                            .pb(px(9.))
                            .flex()
                            .flex_col()
                            .gap(px(6.))
                            .text_size(px(12.))
                            .text_color(theme.text_muted)
                            .child(description)
                            .when(!row.branch.is_empty(), |el| {
                                el.child(format!("Branch {}", row.branch))
                            })
                            .children(cloud_cards::recovery_details(row))
                            .children(row.cleanup_label().map(str::to_owned))
                            .children(error.map(|message| {
                                div().text_color(theme.danger).child(message.to_owned())
                            }))
                            .when(row.cleanup_failed(), |el| {
                                el.child(
                                    action(
                                        "cloud-archive-cleanup",
                                        "Retry local cleanup",
                                        false,
                                        blocked,
                                        Action::Cleanup(id.clone()),
                                    )
                                    .w_auto(),
                                )
                            }),
                    )
                },
            )
            .into_any_element()
    }

    fn archive_cloud_status(&self, theme: &Theme, cx: &Context<Self>) -> Option<AnyElement> {
        if !zuse_viewport::cloud_gate::enabled() || self.source == SourceFilter::Saved {
            return None;
        }
        let message: Option<String> = if let Some(error) =
            self.cloud.error.as_deref().filter(|_| {
                self.cloud_action
                    .as_ref()
                    .and_then(cloud_cards::Action::target)
                    .is_none()
            }) {
            Some(error.to_owned())
        } else if self.source == SourceFilter::Cloud && self.cloud_owner.is_none() {
            let state = self.state.read(cx);
            Some(
                if state.cloud_account.is_none() {
                    "Loading your cloud account…"
                } else if state.cloud_user_id().is_none() {
                    "Sign in through General settings to view cloud archives."
                } else {
                    "Switch to your personal account to view cloud archives."
                }
                .into(),
            )
        } else {
            None
        };
        // Refresh stays available whenever cloud archives are loaded: a row
        // can ask for it (a missing archive source) without any page error.
        if message.is_none() && self.cloud_owner.is_none() {
            return None;
        }
        Some(
            div()
                .pt(px(8.))
                .flex()
                .items_center()
                .gap(px(8.))
                .text_size(px(12.))
                .text_color(theme.text_muted)
                .children(message)
                .when(self.cloud_owner.is_some(), |el| {
                    el.child(
                        archive_button(
                            "cloud-archive-refresh",
                            "Refresh",
                            false,
                            self.cloud.loading || self.cloud.busy,
                            theme,
                        )
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.cloud_card_action(cloud_cards::Action::Refresh, window, cx);
                        })),
                    )
                })
                .into_any_element(),
        )
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
        self.apply_cloud_staleness();
        self.load_cloud(cx);
        if let Some(preview) = &self.preview {
            return preview.clone().into_any_element();
        }
        if let Some(history) = &self.cloud_history {
            return history.clone().into_any_element();
        }
        crate::toast::take_error(&mut self.error, cx);
        zuse_viewport::palette::set_light(Theme::of(cx).appearance.is_light());
        let dialog = self
            .render_cloud_delete_dialog(window.viewport_size(), cx)
            .or_else(|| self.render_delete_all_dialog(window.viewport_size(), cx));
        let theme = Theme::of(cx).for_settings_surface();
        use zuse_viewport::archive_cards as cards;
        if self.search.is_none() {
            let search = cx.new(|cx| {
                zuse_viewport::input::TextInput::new("Search archived chats", window, cx)
            });
            self._search = Some(cx.observe(&search, |_, _, cx| cx.notify()));
            self.search = Some(search);
        }
        let search = self.search.as_ref().unwrap().clone();
        let query = search.read(cx).value(cx).trim().to_lowercase();
        if self.query != query {
            self.query = query.clone();
            self.page = 0;
            self.scroll.reset();
        }
        let projects = self.archive_groups(cx);
        // A project whose last archive went away stops filtering, so it
        // can't silently come back when that project archives again.
        let project_filter = self
            .project
            .clone()
            .filter(|key| projects.keys().any(|(_, id)| id == key));
        // A list still loading can't prove the project gone.
        if project_filter != self.project && !self.cloud.loading {
            self.project = project_filter.clone();
            self.page = 0;
            self.scroll.reset();
        }
        let search_bar = div()
            .flex_1()
            .min_w(px(140.))
            .h(px(28.))
            .px(px(10.))
            .flex()
            .items_center()
            .gap(px(8.))
            .rounded(px(8.))
            .border_1()
            .border_color(theme.border)
            .bg(widgets::block_fill(&theme))
            .child(
                icon(icons::MAGNIFER)
                    .size(px(15.))
                    .text_color(theme.text_muted),
            )
            .child(search);
        let source_menu = self.archive_dropdown(
            ArchiveMenu::Source,
            icons::DITHER_SLIDERS,
            self.source.label().into(),
            [SourceFilter::All, SourceFilter::Saved, SourceFilter::Cloud]
                .into_iter()
                .filter(|source| {
                    *source != SourceFilter::Cloud || zuse_viewport::cloud_gate::enabled()
                })
                .map(|source| {
                    popover::menu_row(&theme, self.source == source, source.label())
                        .id(source.label())
                        .child(source.label())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.source = source;
                            this.reset_archive_filters(cx);
                        }))
                        .into_any_element()
                })
                .collect(),
            &theme,
            cx,
        );
        let project_menu = self.archive_dropdown(
            ArchiveMenu::Project,
            icons::FOLDER,
            projects
                .keys()
                .find(|(_, key)| Some(key) == project_filter.as_ref())
                .map(|(name, _)| name.clone())
                .unwrap_or_else(|| "All projects".into())
                .into(),
            std::iter::once(("All projects".to_owned(), None))
                .chain(
                    projects
                        .keys()
                        .map(|(name, key)| (name.clone(), Some(key.clone()))),
                )
                .map(|(name, project)| {
                    let label = SharedString::from(name);
                    let id = SharedString::from(format!(
                        "archive-project-{}",
                        project.as_deref().unwrap_or("all")
                    ));
                    popover::menu_row(&theme, project_filter == project, id.clone())
                        .id(id)
                        .child(div().truncate().child(label))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.project = project.clone();
                            this.reset_archive_filters(cx);
                        }))
                        .into_any_element()
                })
                .collect(),
            &theme,
            cx,
        );
        let blocked = self.busy.is_some()
            || self.delete_all_task.is_some()
            || self.state.read(cx).engine().is_none();
        let mut groups = Vec::new();
        for ((name, key), mut rows) in projects {
            if project_filter.as_ref().is_some_and(|filter| filter != &key) {
                continue;
            }
            rows.retain(|row| match row {
                ArchiveRow::Saved(entry) => {
                    self.source != SourceFilter::Cloud && entry.matches(&query)
                }
                ArchiveRow::Cloud(row) => {
                    self.source != SourceFilter::Saved
                        && (query.is_empty()
                            || name.to_lowercase().contains(&query)
                            || cloud_cards::matches(row, &query))
                }
            });
            if !rows.is_empty() {
                rows.sort_by_key(|row| std::cmp::Reverse(row.date()));
                groups.push(((name, key), rows));
            }
        }
        let total = groups.iter().map(|(_, rows)| rows.len()).sum();
        let current = archive_page(total, self.page);
        if self.page != current {
            self.page = current;
            self.scroll.reset();
        }
        let first = self.page * ARCHIVE_PAGE_SIZE;
        let mut offset = 0;
        let body = div()
            .flex()
            .flex_col()
            .gap(px(12.))
            .children(groups.into_iter().filter_map(|(name, rows)| {
                let start = first.saturating_sub(offset);
                let end = (first + ARCHIVE_PAGE_SIZE)
                    .saturating_sub(offset)
                    .min(rows.len());
                offset += rows.len();
                (start < end)
                    .then(|| self.archive_group(name, &rows, start..end, blocked, &theme, cx))
            }))
            .when(total == 0, |el| {
                el.child(div().py(px(24.)).text_color(theme.text_muted).child(
                    if self.source == SourceFilter::Cloud && self.cloud.loading {
                        "Loading cloud chats…"
                    } else if query.is_empty()
                        && self.project.is_none()
                        && self.source == SourceFilter::All
                    {
                        "No archived chats yet"
                    } else {
                        "No archived chats match these filters"
                    },
                ))
            })
            .children(cards::pagination(
                total,
                self.page,
                Self::archive_card_action,
                cx,
            ));
        let cloud_status = self.archive_cloud_status(&theme, cx);
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
                            widgets::settings_page_column(window)
                                .child(widgets::section_title(
                                    &theme,
                                    "Archived chats",
                                    "Find archived chats and restore the ones you want to keep using.",
                                ))
                                .child(widgets::section_stack()
                                    .child(div().flex().flex_wrap().items_center()
                                        .gap(px(8.)).child(search_bar).child(source_menu).child(project_menu))
                                    .child(body)
                                    .children(cloud_status)),
                        ),
                )
                .fade_overflow_y(&self.scroll.scroll),
            )
            .children(scrollbar)
            .children(dialog)
            .into_any_element()
    }
}

fn archive_line(theme: &Theme, first: bool, source: &str, id: &str) -> gpui::Stateful<gpui::Div> {
    widgets::card_row(theme, first)
        .id(SharedString::from(format!("archived-{source}-{id}")))
        .py(px(6.))
}

fn archive_date(date: Option<chrono::DateTime<chrono::Utc>>, theme: &Theme) -> gpui::Div {
    div()
        .text_size(px(11.))
        .text_color(theme.text_muted)
        .children(date.map(|date| {
            date.with_timezone(&chrono::Local)
                .format("%-d %b %Y, %H:%M")
                .to_string()
        }))
}

fn archive_button(
    id: &'static str,
    label: &'static str,
    destructive: bool,
    blocked: bool,
    theme: &Theme,
) -> gpui::Stateful<gpui::Div> {
    widgets::action_button(theme, widgets::ActionTone::Filled)
        .id(id)
        .role(gpui::Role::Button)
        .when(!blocked, |el| el.tab_index(0))
        .flex_none()
        .px(px(10.))
        .when(destructive, |el| {
            el.bg(theme.danger.opacity(0.12)).text_color(theme.danger)
        })
        .when(blocked, |el| el.opacity(0.5).cursor_default())
        .child(label)
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

    #[gpui::test]
    fn cloud_archives_reload_when_the_task_list_changes(cx: &mut gpui::TestAppContext) {
        use gpui::AppContext;
        let dir = tempfile::tempdir().unwrap();
        cx.update(|cx| {
            crate::settings::init(Default::default(), dir.path(), cx);
            gpui_base::init(cx);
            cx.set_global(crate::theme::Theme::default());
        });
        let state = cx.new(|_| AppState::new());
        let page = cx.new(|cx| ArchivedPage::new(state.clone(), cx));
        page.update(cx, |page, _| page.cloud_loaded = true);
        // An unrelated state change keeps the loaded archive.
        state.update(cx, |_, cx| cx.notify());
        cx.run_until_parked();
        assert!(page.read_with(cx, |page, _| page.cloud_loaded));
        // A task archived from the sidebar (or a `workspace` account event
        // refreshing the list) reloads it on the next paint.
        state.update(cx, |state, cx| {
            state.acknowledge_cloud_sidebar_archive("task", cx);
            cx.notify();
        });
        cx.run_until_parked();
        assert!(!page.read_with(cx, |page, _| page.cloud_loaded));
        // A cloud action in flight keeps what the page shows, but the change
        // stays pending and reloads once the action ends.
        page.update(cx, |page, _| {
            page.cloud_loaded = true;
            page.cloud.busy = true;
        });
        state.update(cx, |state, cx| {
            state.acknowledge_cloud_sidebar_archive("task", cx);
            cx.notify();
        });
        cx.run_until_parked();
        assert!(page.read_with(cx, |page, _| page.cloud_loaded));
        page.update(cx, |page, _| {
            page.cloud.busy = false;
            page.apply_cloud_staleness();
            assert!(!page.cloud_loaded, "the pending change reloads afterwards");
        });
        // A failed Open leaves its action recorded; it must not block reloads.
        page.update(cx, |page, _| {
            page.cloud_loaded = true;
            page.cloud_action = Some(cloud_cards::Action::Open("task".into()));
        });
        state.update(cx, |state, cx| {
            state.acknowledge_cloud_sidebar_archive("task", cx);
            cx.notify();
        });
        cx.run_until_parked();
        assert!(!page.read_with(cx, |page, _| page.cloud_loaded));
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
