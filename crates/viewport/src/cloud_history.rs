//! Saved cloud history uses a separate viewport with no execution transport.
use super::*;
use crate::localization::message as tr;
use crate::{
    assets::icon,
    settings_style as s,
    shell::{BORDER, MUTED, column, row},
};
use zuse_i18n::messages as msg;
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
    organization: Option<String>,
    before: Option<i64>,
    refresh_view: bool,
    prepended: bool,
    events: Option<UnboundedSender<Value>>,
    pump: Option<Task<()>>,
    tool_reads: HashMap<u64, u64>,
    loaded: bool,
    list: Option<u64>,
    read: Option<(u64, u64)>,
    session: Option<String>,
    action: Option<(u64, String, bool)>,
    confirm_delete: Option<String>,
    confirm_focus: Option<[FocusHandle; 3]>,
    poll: Option<Task<()>>,
    view: Option<Entity<WorkspaceView>>,
    commands: HashMap<String, String>,
    error_target: Option<String>,
}
impl WorkspaceView {
    fn reset_cloud_archive_transport(&mut self) {
        for (id, _) in std::mem::take(&mut self.cloud_archive.tool_reads) {
            self.cancel(id);
            self.pending.remove(&id);
        }
        self.cloud_archive.view = None;
        self.cloud_archive.events = None;
        self.cloud_archive.pump = None;
    }
    pub(super) fn revoke_cloud_archive_history(&mut self) {
        if let Some((id, _)) = self.cloud_archive.read.take() {
            self.cancel(id);
            self.pending.remove(&id);
        }
        for (id, _) in std::mem::take(&mut self.cloud_archive.tool_reads) {
            self.cancel(id);
            self.pending.remove(&id);
        }
        self.cloud_archive.view = None;
        self.cloud_archive.events = None;
        self.cloud_archive.pump = None;
        self.cloud_archive.state.head = None;
        self.cloud_archive.state.rows.clear();
        self.cloud_archive.state.selected = None;
        self.cloud_archive.state.loading = false;
        self.cloud_archive.state.error =
            Some("Cloud access changed. Reopen an authorized workspace.".into());
    }
    pub(super) fn interrupt_cloud_archive(&mut self) {
        self.cloud_archive.poll = None;
        self.cloud_archive.list = None;
        self.cloud_archive.read = None;
        self.cloud_archive.before = None;
        for (id, _) in std::mem::take(&mut self.cloud_archive.tool_reads) {
            self.pending.remove(&id);
        }
        self.reset_cloud_archive_transport();
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
            && self.cloud_archive.action.is_none()
            && self.cloud_archive.read.is_none()
            && self.cloud_archive.state.head.is_none()
        {
            self.cloud_archive.list = Some(self.call(LIST, self.cloud_archive_params(json!({}))));
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
                let owner = value["user"]["id"].as_str().map(str::to_owned);
                let organization = value["orgId"].as_str().map(str::to_owned);
                if owner != self.cloud_archive.owner
                    || organization != self.cloud_archive.organization
                {
                    self.reset_cloud_archive_transport();
                    let mut state = self.cloud_archive.state.clone();
                    state.clear();
                    self.cloud_archive = ArchiveView {
                        owner,
                        organization,
                        state,
                        ..Default::default()
                    };
                    if self.settings
                        && self.settings_section == crate::settings_view::Section::Archived
                    {
                        self.load_cloud_archive();
                    }
                }
                cx.notify();
            }
            return false;
        }
        if let Some(child_id) = self.cloud_archive.tool_reads.remove(&id) {
            let response = if let Some(error) = frame.get("err") {
                json!({"id":child_id,"err":error})
            } else if frame["ok"]["toolOutput"].is_object() {
                json!({"id":child_id,"ok":frame["ok"]["toolOutput"]})
            } else {
                json!({"id":child_id,"err":"Tool output is still syncing. Retry shortly."})
            };
            if let Some(events) = &self.cloud_archive.events {
                let _ = events.unbounded_send(response);
            }
            self.pending.remove(&id);
            return true;
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
        } else if value["accountId"].as_str() != self.cloud_archive.owner.as_deref()
            || value["organizationId"].as_str() != self.cloud_archive.organization.as_deref()
        {
            Some("Your account changed. Reopen this view.".into())
        } else {
            None
        };
        if let Some(error) = error {
            self.cloud_archive.before = None;
            if let Some(child) = &self.cloud_archive.view {
                child.update(cx, |view, cx| {
                    view.cloud.older_loading = false;
                    cx.notify();
                });
            }
            if list {
                self.cloud_archive.error_target = None;
            }
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
                Ok(mut head)
                    if Some(head.session_id.as_str()) == self.cloud_archive.session.as_deref() =>
                {
                    let mut valid = true;
                    if let Some(before) = self.cloud_archive.before.take() {
                        valid = false;
                        if let Some(prior) = &self.cloud_archive.state.head {
                            if value["historyPage"] == true
                                && value["requestedBefore"] == before
                                && prior.workspace_id == head.workspace_id
                                && prior.session_id == head.session_id
                                && prior.cursor == head.cursor
                                && prior.runtime_generation == head.runtime_generation
                                && prior.older_message_sequence == Some(before)
                                && head
                                    .older_message_sequence
                                    .is_none_or(|n| n > 0 && n < before)
                            {
                                if let (
                                    zuse_model::TranscriptFrame::Reset { reset: old },
                                    zuse_model::TranscriptFrame::Reset { reset: new },
                                ) = (&prior.transcript.frame, &head.transcript.frame)
                                {
                                    let mut entries = new.as_ref().clone();
                                    entries.extend(old.iter().cloned());
                                    head.transcript.replay_baseline =
                                        Some(zuse_model::TranscriptBaseline::capture(&entries));
                                    head.transcript.frame = zuse_model::TranscriptFrame::Reset {
                                        reset: entries.into(),
                                    };
                                    valid = true;
                                    self.cloud_archive.prepended = true;
                                }
                            }
                        }
                    }
                    if valid && self.cloud_archive.state.receive(revision, head) {
                        self.cloud_archive.refresh_view = true;
                    } else {
                        self.cloud_archive.state.error =
                            Some("Saved history changed. Reopen the conversation.".into());
                    }
                    if let Some(child) = &self.cloud_archive.view {
                        child.update(cx, |view, cx| {
                            view.cloud.older_loading = false;
                            cx.notify();
                        });
                    }
                }
                _ => {
                    self.cloud_archive.state.error = Some("Saved cloud history is invalid.".into())
                }
            }
        } else if let Some((_, workspace, deleting)) = action {
            if value["workspace"]["workspaceId"].as_str() != Some(workspace.as_str()) {
                self.cloud_archive.state.error =
                    Some("This request could not be verified. Refresh to check its state.".into());
                cx.notify();
                return true;
            }
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
            self.cloud_archive.loaded = false;
            self.load_cloud_archive();
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
        if self.cloud_archive.action.is_some() || self.cloud_archive.owner.is_none() {
            return;
        }
        self.cloud_archive.poll = None;
        self.cloud_archive.confirm_delete = None;
        let command = self
            .cloud_archive
            .commands
            .entry(format!("{method}:{id}"))
            .or_insert_with(|| uuid::Uuid::new_v4().to_string())
            .clone();
        self.cloud_archive.state.busy = true;
        self.cloud_archive.state.error = None;
        self.cloud_archive.error_target = Some(id.into());
        let req = self.call(
            method,
            self.cloud_archive_params(json!({"workspaceId":id,"commandId":command})),
        );
        self.cloud_archive.action = Some((req, id.into(), true));
        cx.notify();
    }
    fn open_cloud_archive(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.cloud_archive.read.is_some()
            || self.cloud_archive.state.busy
            || self.cloud_archive.owner.is_none()
        {
            return;
        }
        if let Some(revision) = self.cloud_archive.state.select(id) {
            self.cloud_archive.poll = None;
            self.reset_cloud_archive_transport();
            self.cloud_archive.error_target = Some(id.into());
            self.cloud_archive.session = self
                .cloud_archive
                .state
                .rows
                .iter()
                .find(|row| row.workspace_id == id)
                .map(|row| row.initial_session_id.clone());
            let req = self.call(
                READ,
                self.cloud_archive_params(
                    json!({"workspaceId":id,"sessionId":self.cloud_archive.session,"incremental":true}),
                ),
            );
            self.cloud_archive.read = Some((req, revision));
            cx.notify();
        }
    }
    fn select_cloud_archive_session(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.cloud_archive.read.is_some()
            || self.cloud_archive.action.is_some()
            || self.cloud_archive.owner.is_none()
        {
            return;
        }
        let Some(head) = self.cloud_archive.state.head.as_ref() else {
            return;
        };
        if head.session_id == id || !head.conversations.iter().any(|chat| chat.id == id) {
            return;
        }
        let workspace = head.workspace_id.clone();
        self.reset_cloud_archive_transport();
        self.cloud_archive.state.revision = self.cloud_archive.state.revision.wrapping_add(1);
        self.cloud_archive.state.loading = true;
        self.cloud_archive.state.error = None;
        self.cloud_archive.session = Some(id.into());
        let req = self.call(
            READ,
            self.cloud_archive_params(
                json!({"workspaceId":workspace,"sessionId":id,"incremental":true}),
            ),
        );
        self.cloud_archive.read = Some((req, self.cloud_archive.state.revision));
        cx.notify();
    }
    fn unarchive_cloud(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.cloud_archive.action.is_some()
            || self.cloud_archive.owner.is_none()
            || !self
                .cloud_archive
                .state
                .rows
                .iter()
                .any(|r| r.workspace_id == id && r.state == "archived")
        {
            return;
        }
        self.cloud_archive.poll = None;
        let command = self
            .cloud_archive
            .commands
            .entry(id.into())
            .or_insert_with(|| uuid::Uuid::new_v4().to_string())
            .clone();
        self.cloud_archive.state.error = None;
        self.cloud_archive.error_target = Some(id.into());
        self.cloud_archive.state.busy = true;
        let req = self.call(
            UNARCHIVE,
            self.cloud_archive_params(json!({"workspaceId":id,"commandId":command})),
        );
        self.cloud_archive.action = Some((req, id.into(), false));
        cx.notify();
    }
    fn cloud_archive_params(&self, mut params: Value) -> Value {
        params["expectedAccountId"] = json!(self.cloud_archive.owner);
        params["expectedOrganizationId"] = json!(self.cloud_archive.organization);
        params
    }
    fn cloud_card_action(
        &mut self,
        action: crate::cloud_archive_cards::Action,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use crate::cloud_archive_cards::Action;
        if self.cloud_archive.owner.is_none()
            || self.cloud_archive.list.is_some()
            || self.cloud_archive.read.is_some()
            || self.cloud_archive.action.is_some()
        {
            return;
        }
        if let Some(id) = action.target() {
            if !self
                .cloud_archive
                .state
                .rows
                .iter()
                .any(|row| row.workspace_id == id && row.archive_visible())
            {
                return;
            }
        }
        match action {
            Action::Refresh => {
                self.cloud_archive.loaded = false;
                self.cloud_archive.state.error = None;
                self.cloud_archive.error_target = None;
                self.load_cloud_archive();
            }
            Action::Open(id) => self.open_cloud_archive(&id, cx),
            Action::Restore(id) => self.unarchive_cloud(&id, cx),
            Action::Delete(id) => {
                let focus = [cx.focus_handle(), cx.focus_handle(), cx.focus_handle()];
                window.focus(&focus[1], cx);
                self.cloud_archive.confirm_focus = Some(focus);
                self.cloud_archive.confirm_delete = Some(id);
            }
            Action::RetryDelete(id) => self.delete_cloud(&id, cx),
            Action::Cleanup(id) => self.mutate_cloud(&id, CLEANUP_MIRROR, cx),
        }
        cx.notify();
    }
    pub(super) fn cloud_archive_list(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        use crate::cloud_archive_cards as cards;
        let access = if self.cloud_account.is_null() {
            cards::Access::Loading
        } else if !self.cloud_account["user"].is_object() {
            cards::Access::SignedOut
        } else {
            cards::Access::Ready
        };
        let pending = self.cloud_archive.action.as_ref().map(|(req, id, _)| {
            match self.pending.get(req).map(String::as_str) {
                Some(CLEANUP_MIRROR) => cards::Action::Cleanup(id.clone()),
                Some(DELETE) => cards::Action::Delete(id.clone()),
                _ => cards::Action::Restore(id.clone()),
            }
        });
        cards::panel(&self.cloud_archive.state.rows, &self.archive_search.read(cx).value(cx), cards::State {
            access,loading:self.cloud_archive.list.is_some(),busy:self.cloud_archive.state.busy,
            opening:self.cloud_archive.read.and(self.cloud_archive.state.selected.as_deref()),
            pending:pending.as_ref(),error:self.cloud_archive.state.error.as_deref(),
            error_target:self.cloud_archive.error_target.as_deref(),
        }, Self::cloud_card_action,cx)
            .when_some(self.cloud_archive.confirm_delete.clone(), |view, id| {
                let focus = self.cloud_archive.confirm_focus.as_ref().unwrap();
                let card = s::card()
                    .id("cloud-delete-dialog")
                    .w(px(420.))
                    .p_5()
                    .role(Role::Dialog)
                    .aria_label(tr(cx, msg::native::delete_cloud_chat))
                    .child(div().text_size(px(18.)).child(tr(cx, msg::native::delete_cloud_chat)))
                    .child(s::description_text(
                        tr(cx, msg::native::this_permanently_removes_all_conversations_cloud_files_and_recovery_backups_for_cb2de551)
                    ).mt_3())
                    .child(
                        row().mt_4().gap_3().justify_end()
                            .child(s::button("cloud-delete-cancel", tr(cx, msg::common::cancel))
                                .track_focus(&focus[1])
                                .tab_index(0)
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.cloud_archive.confirm_delete = None;
                                    window.focus(&this.settings_nav_focus, cx);
                                    cx.notify();
                                })))
                            .child(s::button("cloud-delete-confirm", tr(cx, msg::native::delete_permanently))
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
                self.open_global_archive(chat, window, cx);
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
                        tr(cx, msg::native::saved_chats),
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
            let (requests, mut receiver) = futures::channel::mpsc::unbounded::<Value>();
            let (send, events) = futures::channel::mpsc::unbounded();
            self.cloud_archive.events = Some(send);
            let child = cx.new(|cx| {
                let mut view = Self::new(Transport { requests, events }, window, cx);
                view.install_cached_history(&head);
                view
            });
            self.cloud_archive.view = Some(child);
            self.cloud_archive.refresh_view = true;
            self.cloud_archive.pump=Some(cx.spawn(async move |this,cx| {
                while let Some(request)=receiver.next().await {
                    if this.update(cx,|this,cx| {
                        if request["ui"]=="cloudHistoryOlder" && this.cloud_archive.read.is_none() {
                            if let Some(head)=&this.cloud_archive.state.head {
                                if let Some(before)=head.older_message_sequence {
                                    let body=this.cloud_archive_params(json!({"workspaceId":head.workspace_id,"sessionId":head.session_id,"beforeSequence":before,"cursor":head.cursor,"incremental":true}));
                                    let id=this.call(READ,body);this.cloud_archive.before=Some(before);this.cloud_archive.read=Some((id,this.cloud_archive.state.revision));cx.notify();
                                }
                            }
                        } else if request["method"]==methods::FETCH_TOOL_BLOB {
                            if let (Some(child_id),Some(head))=(request["id"].as_u64(),this.cloud_archive.state.head.as_ref()) {
                                let params=&request["params"];
                                let body=this.cloud_archive_params(json!({"workspaceId":head.workspace_id,"sessionId":head.session_id,"toolBlobRef":params["blobRef"],"offset":params["offset"]}));
                                let id=this.call(READ,body);this.cloud_archive.tool_reads.insert(id,child_id);
                            }
                        }
                    }).is_err(){break;}
                }
            }));
        }
        if std::mem::take(&mut self.cloud_archive.refresh_view) {
            let child = self.cloud_archive.view.as_ref().unwrap().clone();
            let anchor = std::mem::take(&mut self.cloud_archive.prepended).then(|| {
                let view = child.read(cx);
                (
                    view.transcript_scroll.clone(),
                    view.transcript_scroll.max_offset().y,
                    view.transcript_scroll.offset(),
                )
            });
            child.update(cx, |view, cx| {
                view.install_cached_history(&head);
                view.cloud.older_before = head.older_message_sequence;
                view.cloud.older_loading = false;
                cx.notify();
            });
            if let Some((scroll, height, offset)) = anchor {
                window.on_next_frame(move |_, cx| {
                    scroll.set_offset(point(offset.x, offset.y - (scroll.max_offset().y - height)));
                    child.update(cx, |_, cx| cx.notify());
                });
            }
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
                        s::button("cloud-history-back", tr(cx, msg::chat::checkpoints_back))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.cloud_archive.state.back();
                                this.reset_cloud_archive_transport();
                                this.cloud_archive.loaded = false;
                                this.load_cloud_archive();
                                cx.notify();
                            })),
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
                            .child(tr(cx, msg::chat::computer_picker_cloud_sandbox)),
                    ),
            )
            .child(crate::cloud_archive_cards::sessions(
                &head.conversations,
                &head.session_id,
                self.cloud_archive.read.is_some() || self.cloud_archive.state.busy,
                |this, id, _, cx| this.select_cloud_archive_session(&id, cx),
                cx,
            ))
            .child(if self.cloud_archive.read.is_some() && self.cloud_archive.before.is_none() {
                div()
                    .flex_1()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(rgb(MUTED))
                    .child(tr(cx, msg::native::loading_saved_conversation))
                    .into_any_element()
            } else {
                self.cloud_archive.view.clone().unwrap().into_any_element()
            })
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
                            .child(tr(cx, msg::chat::archived_chats_page_this_chat_is_archived))
                            .when_some(self.cloud_archive.state.error.clone(), |v, error| {
                                v.child(error)
                            }),
                    )
                    .child(
                        s::button(
                            "cloud-history-unarchive",
                            if self.cloud_archive.state.busy {
                                "Restoring…"
                            } else {
                                "Restore chat"
                            },
                        )
                        .on_click(cx.listener(move |this, _, _, cx| this.unarchive_cloud(&id, cx))),
                    ),
            )
            .into_any_element()
    }
}
