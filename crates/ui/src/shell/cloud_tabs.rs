//! A cloud chat's tab strip and the cloud new-chat canvas.
//!
//! The strip belongs to one cloud chat (one computer) and lists only its open
//! conversations, as a local thread's strip lists its own: plus adds a
//! conversation on the same computer. A new chat (New thread, the sidebar's
//! new-chat entries) is a standalone canvas without a strip, whose send
//! creates a new cloud chat on a new computer. A canvas whose chat is still
//! being created is set aside when the user moves on: the chat keeps being
//! created, keeps its sidebar row and reopens from it.
use super::cloud_conversation::CloudConversation;
use super::*;

struct CloudDraft {
    key: String,
    project: Option<serde_json::Value>,
    choice: Option<crate::pickers::CloudCanvasChoices>,
    device: Option<String>,
    space: Option<String>,
}
impl Default for CloudDraft {
    fn default() -> Self {
        Self {
            key: format!("cloud-canvas:{}", uuid::Uuid::new_v4()),
            project: None,
            choice: None,
            device: None,
            space: None,
        }
    }
}
#[derive(Default)]
pub(super) struct CloudTabs {
    /// Open cloud conversations, each listed in its own chat's strip. A chat
    /// left with a single tab is forgotten once another chat opens (its row
    /// opens it again as it was); one with tabs added keeps them.
    views: Vec<Entity<CloudConversation>>,
    scope: Option<(String, Option<String>)>,
    /// The cloud new-chat canvas, in front when no chat is.
    canvas: Option<CloudDraft>,
    /// Canvases whose new chat is still being created, set aside.
    launches: Vec<CloudDraft>,
}
impl CloudTabs {
    pub(super) fn retained_index(
        &self,
        target: &crate::state::CloudTarget,
        cx: &App,
    ) -> Option<usize> {
        self.views.iter().position(|view| {
            let current = &view.read(cx).target;
            current.subject == target.subject
                && current.organization == target.organization
                && current.workspace == target.workspace
                && current.initial_session == target.initial_session
        })
    }
    pub(super) fn composer_for_session(&self, key: &str, cx: &App) -> Option<Entity<Composer>> {
        self.views.iter().find_map(|view| {
            let view = view.read(cx);
            (view.target.initial_session == key).then(|| view.composer.clone())
        })
    }
    fn foreign_scope(&self, scope: Option<&(String, Option<String>)>, cx: &App) -> bool {
        self.views.iter().any(|view| {
            let t = &view.read(cx).target;
            scope != Some(&(t.subject.clone(), t.organization.clone()))
        })
    }
    /// Indices of `workspace`'s open conversations, in tab order.
    fn chat_tabs(&self, workspace: &str, cx: &App) -> Vec<usize> {
        self.views
            .iter()
            .enumerate()
            .filter(|(_, view)| view.read(cx).target.workspace == workspace)
            .map(|(index, _)| index)
            .collect()
    }
    /// Another chat is opening: forget the chats left with a single tab.
    fn forget_single_tab_chats(&mut self, opening: &str, cx: &App) {
        let mut counts = std::collections::HashMap::<String, usize>::new();
        for view in &self.views {
            *counts
                .entry(view.read(cx).target.workspace.clone())
                .or_default() += 1;
        }
        self.views.retain(|view| {
            let workspace = &view.read(cx).target.workspace;
            workspace == opening || counts.get(workspace).copied().unwrap_or_default() > 1
        });
    }
}
impl Shell {
    pub(super) fn cloud_tab_navigation_ready(&mut self, cx: &mut Context<Self>) -> bool {
        self.composer.update(cx, |composer, cx| {
            composer.cloud_canvas_navigation_ready(cx)
        })
    }
    /// Whether the new-chat canvas is in front (not a chat).
    fn cloud_canvas_in_front(&self, cx: &App) -> bool {
        self.cloud_conversation.is_none() && self.state.read(cx).selected_chat.is_none()
    }
    fn stash_cloud_canvas(&mut self, cx: &mut Context<Self>) {
        if self.cloud_tabs.canvas.is_none() || !self.cloud_canvas_in_front(cx) {
            return;
        }
        let canvas_key = self
            .composer
            .update(cx, |composer, cx| composer.ensure_cloud_canvas_key(cx));
        if let Some(draft) = self.cloud_tabs.canvas.as_mut() {
            let state = self.state.read(cx);
            draft.project = state.cloud_project.clone();
            draft.device = state.selected_device.clone();
            draft.space = state.selected_space.clone();
            let composer = self.composer.read(cx);
            draft.key = canvas_key;
            draft.choice = Some(composer.pickers().read(cx).cloud_canvas_choices(cx));
        }
    }
    pub(super) fn track_current_cloud_canvas(&mut self, cx: &mut Context<Self>) {
        if !self.cloud_canvas_in_front(cx) || self.state.read(cx).cloud_project.is_none() {
            return;
        }
        let scope = {
            let state = self.state.read(cx);
            state.cloud_user_id().map(|id| {
                (
                    id.to_owned(),
                    state.cloud_organization_id().map(str::to_owned),
                )
            })
        };
        if scope.is_none() {
            return;
        }
        if self.cloud_tabs.canvas.is_none() {
            self.cloud_tabs.canvas = Some(CloudDraft::default());
        }
        self.cloud_tabs.scope = scope;
        self.stash_cloud_canvas(cx);
    }
    pub(super) fn retains_cloud_canvas(&self, key: &str) -> bool {
        self.cloud_tabs
            .canvas
            .iter()
            .chain(&self.cloud_tabs.launches)
            .any(|draft| draft.key == key)
    }
    /// The canvas makes way for another: a chat it is still creating goes on
    /// in the background (its sidebar row reopens it); an unsent draft goes.
    fn set_cloud_canvas_aside(&mut self, cx: &mut Context<Self>) {
        // Only the choices are read here: the new canvas's project, device
        // and space may already be in the state.
        let choice = self.cloud_canvas_in_front(cx).then(|| {
            self.composer
                .read(cx)
                .pickers()
                .read(cx)
                .cloud_canvas_choices(cx)
        });
        let Some(mut draft) = self.cloud_tabs.canvas.take() else {
            return;
        };
        if choice.is_some() {
            draft.choice = choice;
        }
        if self.composer.read(cx).has_cloud_launch(&draft.key) {
            self.cloud_tabs.launches.push(draft);
        } else {
            self.composer.update(cx, |composer, cx| {
                composer.discard_cloud_canvas_keys(&[draft.key], cx)
            });
        }
    }
    /// A fresh new-chat canvas: with Cloud as its destination, its send
    /// creates a new cloud chat on a new computer.
    pub(super) fn initialize_new_cloud_canvas(&mut self, cx: &mut Context<Self>) {
        self.set_cloud_canvas_aside(cx);
        let state = self.state.read(cx);
        if state.cloud_project.is_none() {
            self.composer
                .update(cx, |composer, cx| composer.set_cloud_canvas_key(None, cx));
            return;
        }
        let scope = state.cloud_user_id().map(|id| {
            (
                id.to_owned(),
                state.cloud_organization_id().map(str::to_owned),
            )
        });
        let draft = CloudDraft {
            project: state.cloud_project.clone(),
            device: state.selected_device.clone(),
            space: state.selected_space.clone(),
            ..CloudDraft::default()
        };
        self.cloud_tabs.scope = scope;
        let key = draft.key.clone();
        let project = draft.project.clone();
        self.cloud_tabs.canvas = Some(draft);
        self.composer.update(cx, |composer, cx| {
            composer.set_cloud_canvas_key(Some(key), cx);
            composer
                .pickers()
                .update(cx, |pickers, cx| pickers.start_cloud_canvas(project, cx));
        });
    }
    pub(super) fn sync_cloud_tab_scope(&mut self, cx: &mut Context<Self>) {
        let current = {
            let state = self.state.read(cx);
            state.cloud_user_id().map(|id| {
                (
                    id.to_owned(),
                    state.cloud_organization_id().map(str::to_owned),
                )
            })
        };
        let foreign = self
            .cloud_tabs
            .scope
            .as_ref()
            .is_some_and(|scope| Some(scope) != current.as_ref())
            || self.cloud_tabs.foreign_scope(current.as_ref(), cx)
            || self.cloud_conversation.as_ref().is_some_and(|view| {
                let target = &view.read(cx).target;
                current.as_ref() != Some(&(target.subject.clone(), target.organization.clone()))
            });
        if foreign {
            let keys = self
                .cloud_tabs
                .canvas
                .iter()
                .chain(&self.cloud_tabs.launches)
                .map(|draft| draft.key.clone())
                .collect::<Vec<_>>();
            let active = self.cloud_conversation.is_some();
            self.cloud_conversation_events = None;
            self.cloud_conversation = None;
            self.cloud_tabs = CloudTabs::default();
            self.cloud_activity.clear();
            self.composer
                .update(cx, |composer, cx| composer.revoke_cloud_canvases(&keys, cx));
            self.state.update(cx, |state, cx| {
                state.cloud_project = None;
                cx.notify();
            });
            if active {
                self.active_chat.clear();
            }
            cx.notify();
        }
    }
    /// Plus. In a cloud chat it opens another conversation on the same
    /// computer (its checkout, branch and files), as plus does in a local
    /// thread; it never starts another computer. Elsewhere it is New chat.
    pub(super) fn open_new_conversation_tab(&mut self, cx: &mut Context<Self>) {
        self.sync_cloud_tab_scope(cx);
        if !self.cloud_tab_navigation_ready(cx) {
            return;
        }
        if let Some(view) = self.cloud_conversation.clone() {
            self.add_cloud_conversation(view, cx);
            return;
        }
        self.open_new_session(None, cx);
    }
    /// `view` is opening in front: it joins its own chat's strip, and a chat
    /// opened before with a single tab is forgotten.
    pub(super) fn retain_cloud_tab(
        &mut self,
        view: &Entity<CloudConversation>,
        cx: &mut Context<Self>,
    ) {
        self.stash_cloud_canvas(cx);
        let target = view.read(cx).target.clone();
        self.cloud_tabs.scope = Some((target.subject.clone(), target.organization.clone()));
        self.cloud_tabs
            .forget_single_tab_chats(&target.workspace, cx);
        if self.cloud_tabs.retained_index(&target, cx).is_none() {
            self.cloud_tabs.views.push(view.clone());
        }
    }
    /// Open a new conversation on `origin`'s computer as the active tab at
    /// once, from local state: it is the same running computer, so nothing
    /// is provisioned. The server records the conversation in the background
    /// (no computer wake, a few tens of milliseconds); the computer installs
    /// it in the same thread on its first message, which wakes a sleeping
    /// computer like any send.
    fn add_cloud_conversation(
        &mut self,
        origin: Entity<CloudConversation>,
        cx: &mut Context<Self>,
    ) {
        let opened_at = std::time::Instant::now();
        let (target, thread, template) = {
            let view = origin.read(cx);
            (
                view.target.clone(),
                view.thread_id(),
                view.template_chat(cx),
            )
        };
        let Some(thread) = thread else {
            let composer = origin.read(cx).composer.clone();
            composer.update(cx, |composer, cx| {
                composer.show_error(
                    "This cloud chat is still opening. Try again in a moment.",
                    cx,
                )
            });
            return;
        };
        let session = uuid::Uuid::new_v4().to_string();
        let mut chat = template.clone();
        if !chat.is_object() {
            chat = serde_json::json!({});
        }
        chat["id"] = serde_json::json!(session);
        chat["threadId"] = serde_json::json!(thread);
        chat["deviceId"] = serde_json::json!(format!("cloud:{}", target.workspace));
        chat["cwd"] = serde_json::json!(".");
        chat["archived"] = serde_json::json!(false);
        chat["createdAt"] = serde_json::json!(chrono::Utc::now());
        for key in [
            "title",
            "lastMessagePreview",
            "lastMessageAt",
            "harnessSessionId",
            "harnessSessionCwd",
            "parentChatId",
            "lastSeenAt",
        ] {
            chat[key] = serde_json::Value::Null;
        }
        let Ok(chat) = serde_json::from_value::<zuse_protocol::Chat>(chat) else {
            return;
        };
        origin.update(cx, |view, _| view.remember_conversation(chat.clone()));
        self.open_cloud_conversation_tab(&origin, session.clone(), true, cx);
        let mut opened = target.clone();
        opened.initial_session = session.clone();
        let Some(view) = self
            .cloud_tabs
            .retained_index(&opened, cx)
            .map(|index| self.cloud_tabs.views[index].clone())
        else {
            return;
        };
        tracing::info!(workspace = %target.workspace, ms = opened_at.elapsed().as_millis() as u64,
            "Cloud conversation tab ready");
        // One logical command for every attempt: a retry after a lost reply
        // replays the same conversation and never records a second one.
        let request = serde_json::json!({
            "workspaceId": target.workspace,
            "expectedAccountId": target.subject,
            "expectedOrganizationId": target.organization,
            "logicalCommandId": uuid::Uuid::new_v4().to_string(),
            "requestId": 0,
            "method": methods::CREATE_THREAD_SESSION,
            "params": {"chatId": session, "threadId": thread, "templateChat": template},
        });
        view.update(cx, |view, cx| view.start_registering(chat, request, cx));
    }
    /// Open `session`, a conversation on `origin`'s computer, in a tab of its
    /// own (or its existing tab), in front when `focus`.
    pub(super) fn open_cloud_conversation_tab(
        &mut self,
        origin: &Entity<CloudConversation>,
        session: String,
        focus: bool,
        cx: &mut Context<Self>,
    ) {
        let (mut target, row) = {
            let view = origin.read(cx);
            (view.target.clone(), view.sibling_row())
        };
        target.initial_session = session;
        let index = match self.cloud_tabs.retained_index(&target, cx) {
            Some(index) => index,
            None => {
                // One still working in the background comes back as is.
                let view = match self.background_cloud_conversation(&target, cx) {
                    Some(view) => view,
                    None => {
                        self.new_cloud_conversation_view(target, row, serde_json::Value::Null, cx)
                    }
                };
                self.cloud_tabs.views.push(view);
                self.cloud_tabs.views.len() - 1
            }
        };
        if focus {
            self.select_cloud_tab(index, cx);
        }
        cx.notify();
    }
    /// The conversations on the computer in front that no tab shows, most
    /// recent first: the history menu lists them, as a local thread's lists
    /// its closed conversations.
    pub(super) fn closed_cloud_conversations(&self, cx: &App) -> Vec<zuse_protocol::Chat> {
        let Some(view) = &self.cloud_conversation else {
            return Vec::new();
        };
        let view = view.read(cx);
        let open = self
            .cloud_tabs
            .views
            .iter()
            .map(|tab| &tab.read(cx).target)
            .filter(|target| target.workspace == view.target.workspace)
            .map(|target| target.initial_session.clone())
            .collect::<std::collections::HashSet<_>>();
        let mut closed = view
            .conversations(cx)
            .into_iter()
            .filter(|chat| chat.id != view.target.initial_session && !open.contains(&chat.id))
            .collect::<Vec<_>>();
        closed
            .sort_by_key(|chat| std::cmp::Reverse(chat.last_message_at.unwrap_or(chat.created_at)));
        closed
    }
    /// Reopen a conversation on the computer in front from the history menu.
    pub(super) fn restore_cloud_conversation(&mut self, session: String, cx: &mut Context<Self>) {
        if let Some(view) = self.cloud_conversation.clone() {
            self.open_cloud_conversation_tab(&view, session, true, cx);
        }
    }
    pub(super) fn select_cloud_tab(&mut self, index: usize, cx: &mut Context<Self>) {
        self.sync_cloud_tab_scope(cx);
        if index >= self.cloud_tabs.views.len() || !self.cloud_tab_navigation_ready(cx) {
            return;
        }
        let view = self.cloud_tabs.views[index].clone();
        {
            let target = &view.read(cx).target;
            let state = self.state.read(cx);
            if state.cloud_user_id() != Some(target.subject.as_str())
                || state.cloud_organization_id() != target.organization.as_deref()
            {
                return;
            }
        }
        self.stash_cloud_canvas(cx);
        self.cloud_conversation_events = None;
        self.cloud_conversation = Some(view.clone());
        self.state
            .update(cx, |state, cx| state.select_chat(None, cx));
        self.active_chat = view.read(cx).target.initial_session.clone();
        self.subscribe_cloud_conversation(&view, cx);
        self.set_route(Route::Chat, cx);
        self.focus_composer(cx);
        cx.notify();
    }
    /// Bring back the new-chat canvas whose chat is being created under
    /// `key`, standing alone: the canvas in front (if another) makes way.
    pub(super) fn select_cloud_launch(&mut self, key: &str, cx: &mut Context<Self>) {
        self.sync_cloud_tab_scope(cx);
        let is_canvas = self
            .cloud_tabs
            .canvas
            .as_ref()
            .is_some_and(|draft| draft.key == key);
        let set_aside = self
            .cloud_tabs
            .launches
            .iter()
            .position(|draft| draft.key == key);
        if !is_canvas && set_aside.is_none() {
            return;
        }
        if is_canvas && matches!(self.route, Route::Chat) && self.cloud_canvas_in_front(cx) {
            return;
        }
        if !self.cloud_tab_navigation_ready(cx) {
            return;
        }
        if let Some(position) = set_aside {
            let draft = self.cloud_tabs.launches.remove(position);
            self.set_cloud_canvas_aside(cx);
            self.cloud_tabs.canvas = Some(draft);
        }
        let Some(draft) = self.cloud_tabs.canvas.as_ref() else {
            return;
        };
        let project = draft.project.clone();
        let choice = draft.choice.clone();
        let key = draft.key.clone();
        let device = draft.device.clone();
        let space = draft.space.clone();
        self.cloud_conversation_events = None;
        self.cloud_conversation = None;
        self.active_chat.clear();
        self.state.update(cx, |state, cx| {
            state.select_chat(None, cx);
            state.cloud_project = project.clone();
            if let Some(device) = device {
                state.select_device(device, cx);
            }
            state.select_space(space, cx);
        });
        self.composer.update(cx, |composer, cx| {
            composer.set_cloud_canvas_key(Some(key), cx);
            composer
                .pickers()
                .update(cx, |p, cx| p.restore_cloud_canvas(project, choice, cx));
            // Its pending chat creation locks its choices again.
            composer.refreeze_cloud_launch(cx);
        });
        self.set_route(Route::Chat, cx);
        self.focus_composer(cx);
        cx.notify();
    }
    /// The server admitted the chat drafted under `key`. Its canvas becomes
    /// the conversation when the user is on it; otherwise the sidebar lists
    /// the chat, without taking the user away from where they are.
    pub(super) fn open_admitted_cloud_chat(
        &mut self,
        key: &str,
        target: crate::state::CloudTarget,
        row: serde_json::Value,
        launch: serde_json::Value,
        cx: &mut Context<Self>,
    ) {
        self.cloud_tabs.launches.retain(|draft| draft.key != key);
        let is_canvas = self
            .cloud_tabs
            .canvas
            .as_ref()
            .is_some_and(|draft| draft.key == key);
        let in_front = self.cloud_canvas_in_front(cx) && self.composer.read(cx).current_key == key;
        if is_canvas {
            // The chat replaces its canvas; its key stays for the composer.
            self.cloud_tabs.canvas = None;
        }
        if in_front {
            self.open_cloud_conversation(target, row, launch, cx);
        }
    }
    fn close_cloud_tab(&mut self, index: usize, cx: &mut Context<Self>) {
        self.sync_cloud_tab_scope(cx);
        if index >= self.cloud_tabs.views.len() || !self.cloud_tab_navigation_ready(cx) {
            return;
        }
        let view = self.cloud_tabs.views.remove(index);
        let in_front = self
            .cloud_conversation
            .as_ref()
            .is_some_and(|front| front.entity_id() == view.entity_id());
        if in_front {
            let workspace = view.read(cx).target.workspace.clone();
            let siblings = self.cloud_tabs.chat_tabs(&workspace, cx);
            // The neighbour in the same chat comes to the front, as closing a
            // local thread's tab does; closing the last one leaves the chat.
            match siblings
                .iter()
                .find(|sibling| **sibling >= index)
                .or(siblings.last())
            {
                Some(next) => self.select_cloud_tab(*next, cx),
                None => self.open_new_session(None, cx),
            }
        }
        cx.notify();
    }
    pub(super) fn render_cloud_tabs(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        self.sync_cloud_tab_scope(cx);
        // Only a cloud chat has a strip, listing its own conversations; the
        // new-chat canvas stands alone.
        let front = self.cloud_conversation.clone()?;
        if !matches!(self.route, Route::Chat) {
            return None;
        }
        let workspace = front.read(cx).target.workspace.clone();
        let indices = self.cloud_tabs.chat_tabs(&workspace, cx);
        if indices.is_empty() {
            return None;
        }
        let theme = Theme::of(cx).clone();
        // A file or diff tab in front stands in for the conversation, as in
        // a local chat: the chat tab stays listed but not selected.
        let main_tab_active = self.active_main_tab(cx).is_some();
        let main_chips = self.render_main_tab_chips(cx);
        let tabs = indices
            .into_iter()
            .map(|index| {
                let view = &self.cloud_tabs.views[index];
                let title = view.read(cx).title.clone();
                let harness = {
                    let view = view.read(cx);
                    view.harness(
                        self.state
                            .read(cx)
                            .cloud_sidebar_workspace(&view.target.workspace),
                    )
                };
                let current = view.entity_id() == front.entity_id();
                let active = current && !main_tab_active;
                // Mid-turn, the tab trades its agent icon for the activity
                // glyph, like a local chat's tab.
                let working = view.read(cx).agent_working(cx);
                crate::surface_chrome::workspace_tab(
                    ("cloud-conversation-tab", index),
                    active,
                    &theme,
                )
                .flex()
                .flex_none()
                .items_center()
                .min_w_0()
                .w(px(160.))
                .h(px(28.))
                .px(px(9.))
                .gap(px(8.))
                .when(active, |tab| {
                    tab.text_color(super::tabs::active_tab_text_color(&theme))
                })
                .when(!active, |tab| tab.text_color(theme.text_muted))
                .debug_selector(move || format!("cloud-conversation-tab-{index}"))
                .cursor_pointer()
                .text_size(crate::typography::text_md())
                .child(if working {
                    div()
                        .id(("cloud-tab-working", index))
                        .debug_selector(move || format!("cloud-tab-working-{index}"))
                        .size(px(14.))
                        .flex_none()
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(crate::loaders::mini_glyph_spinner(
                            format!("cloud-tab-working-{index}"),
                            2.0,
                            theme.loader_glyph(),
                            cx.entity_id(),
                            cx,
                        ))
                        .into_any_element()
                } else {
                    cloud_tab_icon(harness, index, &theme).into_any_element()
                })
                .child(div().flex_1().min_w_0().truncate().child(title))
                .child(
                    div()
                        .id(("close-cloud-tab", index))
                        .cursor_pointer()
                        .child(
                            icon(icons::CLOSE)
                                .size(px(12.))
                                .text_color(theme.text_muted),
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.close_cloud_tab(index, cx);
                        })),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    if current && main_tab_active {
                        this.activate_main_tab(None, cx);
                    } else {
                        this.select_cloud_tab(index, cx);
                    }
                }))
            })
            .collect::<Vec<_>>();
        // In a cloud chat, plus adds a conversation on the same computer and
        // the clock lists the computer's conversations no tab shows.
        let history = {
            let closed = self.closed_cloud_conversations(cx);
            Some(self.render_conversation_history_button(&closed, &theme, cx))
        };
        let strip = div()
            .id("cloud-conversation-tabs")
            .flex()
            .min_w_0()
            .overflow_x_scroll()
            .items_center()
            .gap(px(4.))
            .children(tabs)
            .child(
                div()
                    .id("cloud-new-tab")
                    .debug_selector(|| "cloud-new-tab".into())
                    .role(gpui::Role::Button)
                    .aria_label("New chat on this computer")
                    .cursor_pointer()
                    .p(px(5.))
                    .rounded(px(7.))
                    .hover(|d| d.bg(theme.muted_fill().opacity(0.6)))
                    .child(icon(icons::PLUS).size(px(14.)).text_color(theme.text_muted))
                    .on_click(cx.listener(|this, _, _, cx| this.open_new_conversation_tab(cx))),
            )
            .children(main_chips);
        Some(
            div()
                .flex()
                .min_w_0()
                .items_center()
                .gap(px(4.))
                .child(strip)
                .children(history)
                .into_any_element(),
        )
    }
}

impl Shell {
    /// Whether an open conversation (in front or in a tab) for `workspace`
    /// has its agent mid-turn.
    pub(super) fn cloud_workspace_working(&self, workspace: &str, cx: &App) -> bool {
        self.cloud_open_activity(workspace, cx)
            .is_some_and(|(working, _)| working)
    }
    /// Whether a cloud chat's computer is up, for its sidebar row's cloud:
    /// an open conversation's live state, else the listed row's. A message
    /// waiting for a computer that is still starting is not "up".
    pub(super) fn cloud_row_online(&self, workspace: &str, cx: &App) -> bool {
        match self.cloud_open_activity(workspace, cx) {
            Some((_, online)) => online,
            None => self
                .state
                .read(cx)
                .cloud_sidebar_workspace(workspace)
                .is_some_and(super::chat_peek::cloud_row_online),
        }
    }
    /// `(agent working, computer online)` as the open conversations of
    /// `workspace` (in front, in a tab, or running in the background) report
    /// it; `None` when none is open.
    pub(super) fn cloud_open_activity(&self, workspace: &str, cx: &App) -> Option<(bool, bool)> {
        // A computer can show several conversations: the app state combines
        // their reports, working when any is and online when any sees it up.
        self.state
            .read(cx)
            .cloud_chat_activity(workspace)
            .live
            .map(|live| (live.working, live.online))
    }
}

/// The mark a cloud tab leads with: the same agent icon a local chat tab
/// shows, the plain chat glyph before the agent is known.
fn cloud_tab_mark(harness: Option<zuse_protocol::HarnessId>) -> (&'static str, Option<gpui::Hsla>) {
    harness.map_or((icons::CHAT_ROUND_LINE, None), |harness| {
        crate::pickers::harness_brand_icon(harness)
    })
}

/// The agent's icon with a small cloud badge on its lower right corner, so a
/// cloud tab reads as the same agent running on a cloud computer.
fn cloud_tab_icon(
    harness: Option<zuse_protocol::HarnessId>,
    index: usize,
    theme: &Theme,
) -> AnyElement {
    let (path, tint) = cloud_tab_mark(harness);
    div()
        .id(("cloud-tab-icon", index))
        .debug_selector(move || format!("cloud-tab-icon-{index}"))
        .relative()
        .flex_none()
        .size(px(16.))
        .flex()
        .items_center()
        .justify_center()
        .child(icons::brand_icon(
            path,
            14.,
            tint.unwrap_or(theme.text_muted),
        ))
        .child(
            div()
                .absolute()
                .right(px(-4.))
                .bottom(px(-3.))
                .size(px(10.))
                .rounded_full()
                .bg(theme.bg)
                .flex()
                .items_center()
                .justify_center()
                .child(
                    icon(icons::DITHER_CLOUD)
                        .size(px(8.))
                        .text_color(theme.text_muted),
                ),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{AppContext, TestAppContext};
    use serde_json::{Value, json};

    #[test]
    fn cloud_tabs_use_the_local_agent_icon_set() {
        use zuse_protocol::HarnessId;
        for harness in [
            HarnessId::Codex,
            HarnessId::ClaudeCode,
            HarnessId::Grok,
            HarnessId::Gemini,
        ] {
            assert_eq!(
                cloud_tab_mark(Some(harness)),
                crate::pickers::harness_brand_icon(harness)
            );
        }
        assert_eq!(cloud_tab_mark(None), (icons::CHAT_ROUND_LINE, None));
    }
    struct Pending;
    #[async_trait::async_trait]
    impl zuse_rpc::RpcService for Pending {
        async fn handle(
            &self,
            method: &str,
            _: Value,
        ) -> Result<zuse_rpc::RpcReply, zuse_rpc::RpcError> {
            // The sidebar's plan and usage reads never answer either: an
            // answer would land from a runtime thread the test can't order.
            if matches!(
                method,
                zuse_rpc::methods::CLOUD_BILLING_ENTITLEMENTS
                    | zuse_rpc::methods::LIST_AGENT_ACCOUNTS
            ) {
                futures::future::pending::<()>().await;
            }
            Ok(zuse_rpc::RpcReply::Stream(Box::pin(
                futures::stream::pending(),
            )))
        }
    }
    fn draft_context() -> Value {
        let mut caps = Vec::new();
        let mut catalogs = serde_json::Map::new();
        for (id, model) in [("boxd", "boxd-model"), ("e2b", "e2b-model")] {
            caps.push(json!({"id":id,"configured":true,"managedBaseReady":true,"commandAdmissionVersion":2,"preservesDisk":true,"preservesMemory":true,"supportsSnapshots":true,"supportsDocker":false,"sizes":[{"id":"default","label":"Standard","vcpus":2,"memoryMib":2048,"diskGib":8}],"unavailableReason":null}));
            catalogs.insert(id.into(),json!({"providers":[{"id":"codex","name":"Codex","defaultModel":model,"models":[{"id":model,"label":model}]}]}));
        }
        json!({"accountId":"owner","ownerId":"owner","organizationId":null,"commandAdmissionVersion":2,"computerCapabilities":{"providers":caps},"providerAgentCatalogs":catalogs})
    }
    fn draft_shell(cx: &mut TestAppContext, dir: &std::path::Path) -> gpui::WindowHandle<Shell> {
        cx.update(|cx| {
            gpui_base::init(cx);
            cx.set_global(Theme::default());
            crate::app_menus::init(cx);
        });
        cx.add_window(|_,cx| {
            let state=cx.new(|_| {let mut state=AppState::new();state.apply_cloud_account(json!({"user":{"id":"owner"},"orgId":null}));state.apply_cloud_sidebar_catalog(&json!({"accountId":"owner","organizationId":null,"ownerId":"owner","projects":[{"projectId":"p","accountId":"owner","providerId":"boxd","repositoryIdentity":"github.com/owner/p"},{"projectId":"q","accountId":"owner","providerId":"e2b","repositoryIdentity":"github.com/owner/q"}]}),&json!({"accountId":"owner","organizationId":null,"workspaces":[]}));state});
            Shell::new(state,EngineBootConfig{target:None,data_dir:dir.into(),ipc_port:0,edge_url:"http://127.0.0.1:1".into(),edge_token:None,org_id:None,workos_client_id:None,default_harness:zuse_protocol::HarnessId::Mock},cx)
        })
    }
    #[gpui::test]
    fn canvas_plus_starts_a_new_chat_and_scope_change_clears_its_draft(cx: &mut TestAppContext) {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let _entered = runtime.enter();
        let dir = tempfile::tempdir().unwrap();
        let window = draft_shell(cx, dir.path());
        window
            .update(cx, |shell, _, cx| {
                shell.open_new_session(None, cx);
                shell.composer.update(cx, |c, cx| {
                    c.pickers().update(cx, |p, cx| {
                        p.seed_cloud_creation_for_test(
                            draft_context(),
                            json!({"projectId":"p","accountId":"owner","providerId":"boxd"}),
                            cx,
                        )
                    });
                    c.pickers().update(cx, |p, cx| {
                        p.seed_cloud_source_for_test(
                            zuse_protocol::creation_source::Resolution::CloudRef {
                                label: "First ref".into(),
                                base_ref: "refs/heads/first".into(),
                                branch: None,
                            },
                            cx,
                        )
                    });
                    c.input
                        .update(cx, |input, cx| input.set_text("First unsent draft", cx));
                    c.attachments.insert(
                        c.current_key.clone(),
                        vec![crate::attachments::StagedAttachment::from_file(
                            "first.txt",
                            b"first".to_vec(),
                        )],
                    );
                    c.appshots
                        .insert(c.current_key.clone(), vec![crate::appshots::tests::shot()]);
                });
                let first_key = shell.composer.read(cx).current_key.clone();
                shell.open_new_conversation_tab(cx);
                let second_key = shell.composer.read(cx).current_key.clone();
                assert_ne!(first_key, second_key);
                assert!(shell.composer.read(cx).input.read(cx).text().is_empty());
                shell.composer.update(cx, |c, cx| {
                    c.pickers().update(cx, |p, cx| {
                        p.seed_cloud_creation_for_test(
                            draft_context(),
                            json!({"projectId":"q","accountId":"owner","providerId":"e2b"}),
                            cx,
                        )
                    });
                    c.pickers().update(cx, |p, cx| {
                        p.seed_cloud_source_for_test(
                            zuse_protocol::creation_source::Resolution::CloudRef {
                                label: "Second ref".into(),
                                base_ref: "refs/heads/second".into(),
                                branch: None,
                            },
                            cx,
                        )
                    });
                    c.input
                        .update(cx, |input, cx| input.set_text("Second unsent draft", cx));
                    c.attachments.insert(
                        c.current_key.clone(),
                        vec![crate::attachments::StagedAttachment::from_file(
                            "second.txt",
                            b"second".to_vec(),
                        )],
                    );
                });
                assert!(
                    shell.cloud_tabs.views.is_empty() && shell.render_cloud_tabs(cx).is_none(),
                    "plus on the new-chat canvas starts another new chat, never a tab"
                );
                assert!(
                    !shell.composer.read(cx).attachments.contains_key(&first_key),
                    "the replaced unsent draft goes, as New chat discards it"
                );
                shell.composer.update(cx, |c, cx| {
                    c.pickers().update(cx, |p, cx| {
                        p.seed_cloud_source_for_test(
                            zuse_protocol::creation_source::Resolution::Issue {
                                number: 23,
                                title: "Issue in second draft".into(),
                                attachment_path: "issue.md".into(),
                                markdown: "Scoped issue body".into(),
                                upload_id: None,
                            },
                            cx,
                        )
                    })
                });
                shell.state.update(cx, |state, _| {
                    state.apply_cloud_account(json!({"user":{"id":"other"},"orgId":null}));
                });
                shell.sync_cloud_tab_scope(cx);
                let c = shell.composer.read(cx);
                assert!(c.input.read(cx).text().is_empty());
                assert!(c.attachments.is_empty());
                assert!(c.appshots.is_empty());
                assert!(
                    c.pickers().read(cx).source_issue(cx).is_none(),
                    "scope change drops retained issue content synchronously"
                );
                assert!(shell.cloud_tabs.views.is_empty());
                assert!(shell.cloud_conversation_events.is_none());
                assert!(shell.cloud_conversation.is_none());
            })
            .unwrap();
        window
            .update(cx, |_, window, _| window.remove_window())
            .unwrap();
    }
    /// New thread while a new cloud chat is still being created leaves it,
    /// as New thread leaves a local chat: a standalone canvas (no strip, no
    /// tab for the pending chat) whose send creates another new chat. The
    /// pending chat keeps its request and sidebar row, and its row brings it
    /// back, alone and intact.
    #[gpui::test]
    fn new_thread_leaves_a_pending_cloud_chat_running_and_its_row_reopens_it(
        cx: &mut TestAppContext,
    ) {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let _entered = runtime.enter();
        let dir = tempfile::tempdir().unwrap();
        let window = draft_shell(cx, dir.path());
        window.update(cx,|shell,_,cx| {
            shell.open_new_session(None,cx);
            shell.track_current_cloud_canvas(cx);
            assert!(shell.render_cloud_tabs(cx).is_none(),"the new-chat canvas has no tab strip");
            let key=shell.composer.read(cx).current_key.clone();
            let request=json!({"idempotencyKey":"one-original-key","projectId":"p","providerId":"e2b","firstMessage":"Keep this task","agent":"mock"});
            shell.composer.update(cx,|composer,cx|{
                composer.seed_pending_cloud_creation_for_test(request.clone());
                composer.pickers().update(cx,|p,cx|p.freeze_cloud_launch(true,cx));
            });
            let rows=shell.pending_launch_rows(cx);
            assert_eq!(rows.len(),1,"the new chat is in the sidebar at once");
            assert_eq!(rows[0]["title"],"Keep this task");
            assert_eq!(rows[0]["projectId"],"p");
            // New thread: a standalone canvas for another new chat.
            shell.open_new_session(None,cx);
            assert!(shell.render_cloud_tabs(cx).is_none(),"no strip holds the pending chat");
            assert!(shell.cloud_conversation.is_none());
            assert!(shell.cloud_tabs.views.is_empty());
            let c=shell.composer.read(cx);
            assert_ne!(c.current_key,key);
            assert!(c.current_key.starts_with("cloud-canvas:"),"its send creates a new cloud chat");
            assert!(c.pending_cloud_creation_for_test().is_none(),"the canvas has its own empty draft");
            assert!(!c.pickers().read(cx).cloud_launch_frozen(),"its model picker opens");
            assert!(c.visible_failure().is_none());
            assert_eq!(shell.state.read(cx).cloud_project.as_ref().unwrap()["projectId"],"p");
            assert!(shell.composer.read(cx).has_cloud_launch(&key),"the pending chat is still being created");
            // Plus on the canvas and another project are new chats too.
            shell.open_new_conversation_tab(cx);
            shell.open_new_cloud_session(json!({"projectId":"q","accountId":"owner"}),cx);
            assert!(shell.render_cloud_tabs(cx).is_none());
            assert!(shell.composer.read(cx).has_cloud_launch(&key),"the launch survives");
            let rows=shell.pending_launch_rows(cx);
            assert_eq!(rows.len(),1,"its sidebar row stays");
            assert_eq!(rows[0]["title"],"Keep this task");
            // The sidebar row brings it back alone, with its request.
            shell.open_cloud_sidebar_row(&format!("{}launch:{key}",crate::shell::CLOUD_ROW_PREFIX),cx);
            assert!(shell.render_cloud_tabs(cx).is_none());
            assert!(shell.cloud_conversation.is_none());
            let c=shell.composer.read(cx);
            assert_eq!(c.current_key,key);
            assert_eq!(c.pending_cloud_creation_for_test(),Some(&request));
            assert!(c.pickers().read(cx).cloud_launch_frozen(),"its choices stay locked while it is created");
            assert_eq!(shell.state.read(cx).cloud_project.as_ref().unwrap()["projectId"],"p");
            assert_eq!(shell.pending_launch_rows(cx).len(),1);
            shell.state.update(cx,|state,_|{state.apply_cloud_account(json!({"user":{"id":"other"},"orgId":null}));});shell.sync_cloud_tab_scope(cx);assert!(shell.composer.read(cx).pending_cloud_creation_for_test().is_none(),"scope revocation retires the prior account's authority instead of retaining it under another account");
            assert!(shell.composer.read(cx).cloud_launch_summaries().is_empty());
        }).unwrap();
        window
            .update(cx, |_, window, _| window.remove_window())
            .unwrap();
    }
    #[gpui::test]
    fn direct_first_cloud_admission_tracks_and_revokes_the_actual_shared_canvas(
        cx: &mut TestAppContext,
    ) {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let _entered = runtime.enter();
        let dir = tempfile::tempdir().unwrap();
        let window = draft_shell(cx, dir.path());
        let controller = crate::state::EngineHandle::from_test_client(zuse_rpc::memory_client(
            std::sync::Arc::new(Pending),
        ));
        window
            .update(cx, |shell, _, cx| {
                assert!(shell.cloud_tabs.views.is_empty());
                shell.state.update(cx, |state, _| {
                    state.cloud_project =
                        Some(json!({"projectId":"p","accountId":"owner","providerId":"boxd"}))
                });
                shell.composer.update(cx, |c, cx| {
                    c.input.update(cx, |input, cx| {
                        input.set_text("Private pending follow-up", cx)
                    });
                    c.attachments.insert(
                        c.current_key.clone(),
                        vec![crate::attachments::StagedAttachment::from_file(
                            "private.txt",
                            b"private file".to_vec(),
                        )],
                    );
                    c.appshots
                        .insert(c.current_key.clone(), vec![crate::appshots::tests::shot()]);
                });
                shell.open_cloud_conversation(
                    crate::state::CloudTarget {
                        controller,
                        subject: "owner".into(),
                        organization: None,
                        workspace: "direct-task".into(),
                        initial_session: "direct-session".into(),
                    },
                    json!({"workspaceId":"direct-task","projectId":"p"}),
                    json!({"firstMessage":"Already admitted"}),
                    cx,
                );
                let key = shell.composer.read(cx).current_key.clone();
                assert!(key.starts_with("cloud-canvas:"));
                assert_eq!(shell.cloud_tabs.canvas.as_ref().unwrap().key, key);
                assert_eq!(
                    shell.composer.read(cx).attachments[&key][0].name,
                    "private.txt"
                );
                shell.state.update(cx, |state, _| {
                    state.apply_cloud_account(json!({"user":{"id":"other"},"orgId":null}));
                });
                shell.sync_cloud_tab_scope(cx);
                let c = shell.composer.read(cx);
                assert!(c.input.read(cx).text().is_empty());
                assert!(c.attachments.is_empty());
                assert!(c.appshots.is_empty());
                assert!(shell.cloud_conversation.is_none());
                assert!(shell.cloud_conversation_events.is_none());
                assert!(shell.cloud_tabs.views.is_empty());
            })
            .unwrap();
        window
            .update(cx, |_, window, _| window.remove_window())
            .unwrap();
    }
    #[gpui::test]
    fn sidebar_cloud_open_does_not_assign_a_selected_local_draft_to_cloud_scope(
        cx: &mut TestAppContext,
    ) {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let _entered = runtime.enter();
        let dir = tempfile::tempdir().unwrap();
        let window = draft_shell(cx, dir.path());
        let controller = crate::state::EngineHandle::from_test_client(zuse_rpc::memory_client(
            std::sync::Arc::new(Pending),
        ));
        window
            .update(cx, |shell, _, cx| {
                shell.state.update(cx, |state, _| {
                    state.selected_chat = Some("local-session".into());
                    state.cloud_project = None;
                });
                shell.composer.update(cx, |c, cx| {
                    c.set_cloud_canvas_key(None, cx);
                    c.input
                        .update(cx, |input, cx| input.set_text("Keep local draft", cx));
                    c.attachments.insert(
                        "local-session".into(),
                        vec![crate::attachments::StagedAttachment::from_file(
                            "local.txt",
                            b"local file".to_vec(),
                        )],
                    );
                });
                shell.open_cloud_conversation(
                    crate::state::CloudTarget {
                        controller,
                        subject: "owner".into(),
                        organization: None,
                        workspace: "sidebar-task".into(),
                        initial_session: "sidebar-session".into(),
                    },
                    json!({"workspaceId":"sidebar-task","projectId":"p"}),
                    json!({}),
                    cx,
                );
                shell
                    .composer
                    .update(cx, |c, cx| c.set_cloud_canvas_key(None, cx));
                shell.state.update(cx, |state, _| {
                    state.apply_cloud_account(json!({"user":{"id":"other"},"orgId":null}));
                });
                shell.sync_cloud_tab_scope(cx);
                shell.state.update(cx, |state, _| {
                    state.selected_chat = Some("local-session".into())
                });
                shell
                    .composer
                    .update(cx, |c, cx| c.set_cloud_canvas_key(None, cx));
                let c = shell.composer.read(cx);
                assert_eq!(c.input.read(cx).text(), "Keep local draft");
                assert_eq!(c.attachments["local-session"][0].name, "local.txt");
            })
            .unwrap();
        window
            .update(cx, |_, window, _| window.remove_window())
            .unwrap();
    }
    #[gpui::test]
    fn delayed_appshot_keeps_original_cloud_canvas_and_rejects_closed_or_revoked_destination(
        cx: &mut TestAppContext,
    ) {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let _entered = runtime.enter();
        let dir = tempfile::tempdir().unwrap();
        let window = draft_shell(cx, dir.path());
        window
            .update(cx, |shell, window, cx| {
                shell.settings.appshot_destination = crate::appshots::AppshotDestination::Automatic;
                shell.open_new_session(None, cx);
                let identity = shell.appshot_capture_identity(cx);
                let first = identity.canvas.clone().unwrap();
                assert!(shell.receive_scoped_appshot(
                    identity.clone(),
                    crate::appshots::tests::shot(),
                    window,
                    cx
                ));
                assert_eq!(shell.composer.read(cx).appshots[&first].len(), 1);
                // A new chat replaces the canvas: a late shot for the old one
                // is refused, not staged on the new one.
                shell.open_new_session(None, cx);
                let second = shell.composer.read(cx).current_key.clone();
                assert_ne!(first, second);
                assert!(!shell.receive_scoped_appshot(
                    identity,
                    crate::appshots::tests::shot(),
                    window,
                    cx
                ));
                assert!(!shell.composer.read(cx).appshots.contains_key(&second));
                let identity = shell.appshot_capture_identity(cx);
                shell.state.update(cx, |state, _| {
                    state.apply_cloud_account(json!({"user":{"id":"other"},"orgId":null}));
                });
                assert!(!shell.receive_scoped_appshot(
                    identity,
                    crate::appshots::tests::shot(),
                    window,
                    cx
                ));
                assert!(shell.composer.read(cx).appshots.is_empty());
            })
            .unwrap();
        window
            .update(cx, |_, window, _| window.remove_window())
            .unwrap();
    }
    struct TabsOnly {
        shell: Entity<Shell>,
        _dir: tempfile::TempDir,
    }
    impl Render for TabsOnly {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            div().w(px(900.)).h(px(100.)).children(
                self.shell
                    .update(cx, |shell, cx| shell.render_cloud_tabs(cx)),
            )
        }
    }
    #[gpui::test]
    fn single_cloud_tab_paints_clickable_plus_with_sidebar_open(cx: &mut TestAppContext) {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let _entered = runtime.enter();
        let dir = tempfile::tempdir().unwrap();
        let shell_window = draft_shell(cx, dir.path());
        let shell = shell_window.entity(cx).unwrap();
        let controller = crate::state::EngineHandle::from_test_client(zuse_rpc::memory_client(
            std::sync::Arc::new(Pending),
        ));
        shell.update(cx, |shell, cx| {
            shell.settings.sidebar_collapsed = false;
            shell.open_new_session(None, cx);
            assert!(
                shell.render_cloud_tabs(cx).is_none(),
                "the new-chat canvas has no strip"
            );
            shell.open_cloud_conversation(
                crate::state::CloudTarget {
                    controller,
                    subject: "owner".into(),
                    organization: None,
                    workspace: "task1".into(),
                    initial_session: "session1".into(),
                },
                json!({"workspaceId":"task1","chatId":"thread1","projectId":"p"}),
                json!({"firstMessage":"First task"}),
                cx,
            );
        });
        let handle = cx.add_window(|_, _| TabsOnly {
            shell: shell.clone(),
            _dir: dir,
        });
        cx.update_window(handle.into(), |_, window, cx| window.draw(cx).clear())
            .unwrap();
        {
            let mut visual = gpui::VisualTestContext::from_window(handle.into(), cx);
            let plus = visual
                .debug_bounds("cloud-new-tab")
                .expect("single cloud tab must paint its explicit plus");
            visual.simulate_click(plus.center(), gpui::Modifiers::default());
        }
        shell.read_with(cx, |shell, cx| {
            assert_eq!(shell.cloud_tabs.views.len(), 2);
            assert_eq!(
                shell
                    .cloud_conversation
                    .as_ref()
                    .unwrap()
                    .read(cx)
                    .target
                    .workspace,
                "task1",
                "plus adds a conversation on the same computer"
            );
        });
        handle
            .update(cx, |_, window, _| window.remove_window())
            .unwrap();
        shell_window
            .update(cx, |_, window, _| window.remove_window())
            .unwrap();
    }
    #[gpui::test]
    fn each_cloud_chat_shows_its_own_tabs_and_new_thread_leaves_them(cx: &mut TestAppContext) {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let _entered = runtime.enter();
        let dir = tempfile::tempdir().unwrap();
        cx.update(|cx| {
            gpui_base::init(cx);
            cx.set_global(Theme::default());
            crate::app_menus::init(cx);
            crate::history::init(
                Default::default(),
                Default::default(),
                Default::default(),
                Default::default(),
                cx,
            );
        });
        let controller = crate::state::EngineHandle::from_test_client(zuse_rpc::memory_client(
            std::sync::Arc::new(Pending),
        ));
        let window = cx.add_window(|_,cx| {
            let state = cx.new(|_| {
                let mut state = AppState::new();
                state.apply_cloud_account(json!({"user":{"id":"owner"},"orgId":null}));
                state.apply_cloud_sidebar_catalog(
                    &json!({"accountId":"owner","organizationId":null,"ownerId":"owner","projects":[{"projectId":"p","accountId":"owner","repositoryIdentity":"github.com/owner/repo"}]}),
                    &json!({"accountId":"owner","organizationId":null,"workspaces":[]}));
                state
            });
            Shell::new(state,EngineBootConfig {
                target:None,data_dir:dir.path().into(),ipc_port:0,edge_url:"http://127.0.0.1:1".into(),
                edge_token:None,org_id:None,workos_client_id:None,default_harness:zuse_protocol::HarnessId::Mock,
            },cx)
        });
        let windows = cx.update(|cx| cx.windows().len());
        window
            .update(cx, |shell, _, cx| {
                let target = crate::state::CloudTarget {
                    controller,
                    subject: "owner".into(),
                    organization: None,
                    workspace: "task1".into(),
                    initial_session: "session1".into(),
                };
                shell.open_cloud_conversation(
                    target.clone(),
                    json!({"workspaceId":"task1","chatId":"thread1","projectId":"p"}),
                    json!({"firstMessage":"First task"}),
                    cx,
                );
                // Plus in a cloud chat opens a conversation on its computer
                // at once, never a draft for another computer.
                shell.open_new_conversation_tab(cx);
                assert_eq!(shell.cloud_tabs.views.len(), 2);
                let added = shell.cloud_conversation.clone().unwrap();
                assert_eq!(added.read(cx).target.workspace, "task1");
                assert_ne!(added.read(cx).target.initial_session, "session1");
                assert!(shell.state.read(cx).cloud_project.is_none());
                let added = added.read(cx).target.initial_session.clone();
                // New thread leaves the chat: a standalone canvas, no strip,
                // whose send creates a new cloud chat on a new computer.
                shell.open_new_session(None, cx);
                assert!(shell.cloud_conversation.is_none());
                assert!(
                    shell.render_cloud_tabs(cx).is_none(),
                    "the new-chat canvas has no tab strip"
                );
                assert!(
                    shell
                        .composer
                        .read(cx)
                        .current_key
                        .starts_with("cloud-canvas:")
                );
                assert_eq!(
                    shell.state.read(cx).cloud_project.as_ref().unwrap()["projectId"],
                    "p"
                );
                // The chat comes back with its own tabs and live state.
                let state = shell.cloud_tabs.views[0].read(cx).state.entity_id();
                shell.state.update(cx, |state, _| {
                    state.selected_chat = Some("local-selection".into())
                });
                shell.open_cloud_conversation(
                    target.clone(),
                    json!({"workspaceId":"task1","projectId":"p"}),
                    json!({}),
                    cx,
                );
                assert!(
                    shell.state.read(cx).selected_chat.is_none(),
                    "retained cloud restoration clears local selection synchronously"
                );
                assert_eq!(
                    shell.conversation_state(cx).entity_id(),
                    state,
                    "switching reuses the live state and transcript"
                );
                assert!(shell.render_cloud_tabs(cx).is_some());
                assert_eq!(strip(shell, cx), vec!["session1".to_owned(), added.clone()]);
                // Another chat shows only its own tabs, never a draft, and
                // its plus stays on its own computer.
                let second = crate::state::CloudTarget {
                    workspace: "task2".into(),
                    initial_session: "session2".into(),
                    ..target.clone()
                };
                shell.open_cloud_conversation(
                    second.clone(),
                    json!({"workspaceId":"task2","chatId":"thread2","projectId":"p"}),
                    json!({"firstMessage":"Second task"}),
                    cx,
                );
                assert_eq!(strip(shell, cx), vec!["session2".to_owned()]);
                shell.open_new_conversation_tab(cx);
                let second_tabs = strip(shell, cx);
                assert_eq!(second_tabs.len(), 2);
                assert_eq!(
                    shell
                        .cloud_conversation
                        .as_ref()
                        .unwrap()
                        .read(cx)
                        .target
                        .workspace,
                    "task2"
                );
                // Back to the first chat: its two tabs, nothing of the other.
                shell.open_cloud_conversation(
                    target.clone(),
                    json!({"workspaceId":"task1","projectId":"p"}),
                    json!({}),
                    cx,
                );
                assert_eq!(strip(shell, cx), vec!["session1".to_owned(), added]);
                shell.open_cloud_conversation(
                    second,
                    json!({"workspaceId":"task2","projectId":"p"}),
                    json!({}),
                    cx,
                );
                assert_eq!(strip(shell, cx), second_tabs);
                // A third chat shows just its own tab.
                let third = crate::state::CloudTarget {
                    workspace: "task3".into(),
                    initial_session: "session3".into(),
                    ..target.clone()
                };
                shell.open_cloud_conversation(
                    third,
                    json!({"workspaceId":"task3","projectId":"p"}),
                    json!({}),
                    cx,
                );
                assert_eq!(strip(shell, cx), vec!["session3".to_owned()]);
                shell.open_new_session(None, cx);
                assert!(shell.render_cloud_tabs(cx).is_none());
                shell.open_cloud_conversation(
                    target,
                    json!({"workspaceId":"task1","projectId":"p"}),
                    json!({"firstMessage":"Private title"}),
                    cx,
                );
                shell.state.update(cx, |state, _| {
                    state.apply_cloud_account(json!({"user":{"id":"other"},"orgId":null}))
                });
                assert!(shell.render_cloud_tabs(cx).is_none());
                assert!(
                    shell.cloud_tabs.views.is_empty(),
                    "another account must never see retained titles"
                );
                assert!(
                    shell.cloud_conversation.is_none(),
                    "foreign active view is dropped synchronously"
                );
                assert!(
                    shell.cloud_conversation_events.is_none(),
                    "foreign Shell subscription is dropped synchronously"
                );
            })
            .unwrap();
        assert_eq!(cx.update(|cx| cx.windows().len()), windows);
    }
    /// The sessions the strip in front lists, in tab order.
    fn strip(shell: &Shell, cx: &App) -> Vec<String> {
        let workspace = shell
            .cloud_conversation
            .as_ref()
            .unwrap()
            .read(cx)
            .target
            .workspace
            .clone();
        shell
            .cloud_tabs
            .chat_tabs(&workspace, cx)
            .into_iter()
            .map(|index| {
                shell.cloud_tabs.views[index]
                    .read(cx)
                    .target
                    .initial_session
                    .clone()
            })
            .collect()
    }
    /// Answers a cloud chat's plus (`CreateThreadSession` through `CloudRpc`)
    /// as the server does, and records every request.
    #[derive(Default)]
    struct Computer {
        seen: std::sync::Mutex<Vec<(String, Value)>>,
        /// Refuse this many conversation registrations first.
        refuse: std::sync::atomic::AtomicUsize,
    }
    #[async_trait::async_trait]
    impl zuse_rpc::RpcService for Computer {
        async fn handle(
            &self,
            method: &str,
            params: Value,
        ) -> Result<zuse_rpc::RpcReply, zuse_rpc::RpcError> {
            self.seen
                .lock()
                .unwrap()
                .push((method.to_owned(), params.clone()));
            if method == methods::CLOUD_RPC && params["method"] == methods::CREATE_THREAD_SESSION {
                use std::sync::atomic::Ordering;
                if self
                    .refuse
                    .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
                    .is_ok()
                {
                    return Ok(zuse_rpc::RpcReply::Stream(Box::pin(futures::stream::once(
                        async { json!({"id":0,"err":"The cloud action was not confirmed."}) },
                    ))));
                }
                let inner = &params["params"];
                let chat = json!({"id":inner["chatId"],"threadId":inner["threadId"],
                    "deviceId":format!("cloud:{}",params["workspaceId"].as_str().unwrap()),
                    "archived":false,"cwd":".","createdAt":"2026-11-23T00:00:00Z"});
                return Ok(zuse_rpc::RpcReply::Stream(Box::pin(futures::stream::once(
                    async move { json!({"id":0,"ok":chat}) },
                ))));
            }
            Ok(zuse_rpc::RpcReply::Stream(Box::pin(
                futures::stream::pending(),
            )))
        }
    }
    /// The owner's request: plus in a cloud chat opens a new conversation on
    /// the same computer (its workspace, branch and files), side by side, as
    /// plus does in a local thread. It never creates another computer. A
    /// closed conversation stays on the computer and reopens from history.
    #[gpui::test]
    fn plus_in_a_cloud_chat_opens_a_conversation_on_the_same_computer(cx: &mut TestAppContext) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _entered = runtime.enter();
        let dir = tempfile::tempdir().unwrap();
        let window = draft_shell(cx, dir.path());
        cx.update(|cx| gpui_tokio::init_from_handle(cx, runtime.handle().clone()));
        let computer = std::sync::Arc::new(Computer::default());
        let controller =
            crate::state::EngineHandle::from_test_client(zuse_rpc::memory_client(computer.clone()));
        window
            .update(cx, |shell, _, cx| {
                shell.open_cloud_conversation(
                    crate::state::CloudTarget {
                        controller,
                        subject: "owner".into(),
                        organization: None,
                        workspace: "task1".into(),
                        initial_session: "session1".into(),
                    },
                    json!({"workspaceId":"task1","chatId":"thread1","initialSessionId":"session1",
                        "projectId":"p","title":"First task"}),
                    Value::Null,
                    cx,
                );
                // The tab opens at once, from local state, before the server
                // has recorded the conversation.
                let clicked = std::time::Instant::now();
                shell.open_new_conversation_tab(cx);
                let ready = clicked.elapsed();
                assert_eq!(shell.cloud_tabs.views.len(), 2);
                let front = shell.cloud_conversation.clone().unwrap();
                assert_ne!(front.read(cx).target.initial_session, "session1");
                assert!(front.read(cx).registering);
                eprintln!("cloud plus -> tab ready: {} us", ready.as_micros());
                assert!(ready < std::time::Duration::from_millis(100));
                // A send in those few milliseconds waits, keeping its draft,
                // so it never reaches the server for an unrecorded chat.
                let composer = front.read(cx).composer.clone();
                composer.update(cx, |composer, cx| {
                    composer.send_for_test("Too early".into(), false, cx);
                    assert!(composer.failure.is_some());
                });
                assert!(
                    computer
                        .seen
                        .lock()
                        .unwrap()
                        .iter()
                        .all(|(method, _)| method != methods::CLOUD_TASK_COMMAND)
                );
            })
            .unwrap();
        let recorded = |cx: &mut TestAppContext| {
            window
                .update(cx, |shell, _, cx| {
                    !shell
                        .cloud_conversation
                        .as_ref()
                        .unwrap()
                        .read(cx)
                        .registering
                })
                .unwrap()
        };
        for _ in 0..400 {
            cx.run_until_parked();
            if recorded(cx) {
                break;
            }
            runtime.block_on(tokio::task::yield_now());
        }
        assert!(recorded(cx), "the server records it in the background");
        let creates = || {
            computer
                .seen
                .lock()
                .unwrap()
                .iter()
                .filter(|(method, params)| {
                    method == methods::CLOUD_RPC
                        && params["method"] == methods::CREATE_THREAD_SESSION
                })
                .map(|(_, params)| params.clone())
                .collect::<Vec<_>>()
        };
        let create = creates();
        assert_eq!(create.len(), 1);
        assert_eq!(create[0]["workspaceId"], "task1");
        assert_eq!(create[0]["params"]["threadId"], "thread1");
        let session = create[0]["params"]["chatId"].as_str().unwrap().to_owned();
        assert!(
            computer
                .seen
                .lock()
                .unwrap()
                .iter()
                .all(|(method, _)| method != methods::CLOUD_CREATE),
            "plus never creates another computer"
        );
        window
            .update(cx, |shell, _, cx| {
                let front = shell.cloud_conversation.clone().unwrap();
                let view = front.read(cx);
                assert_eq!(view.target.workspace, "task1");
                assert_eq!(view.target.initial_session, session);
                assert_eq!(view.title, "New chat");
                assert!(
                    view.composer.read(cx).failure.is_none(),
                    "the held send's notice clears once the chat is recorded"
                );
                assert_eq!(view.thread_id().as_deref(), Some("thread1"));
                assert_eq!(
                    shell.cloud_tabs.views[1].entity_id(),
                    shell.cloud_conversation.as_ref().unwrap().entity_id()
                );
                assert!(shell.closed_cloud_conversations(cx).is_empty());
                assert!(shell.render_cloud_tabs(cx).is_some());

                // Closing the new tab keeps its conversation on the computer:
                // the first conversation's history lists it, and it reopens
                // in a tab without registering anything again.
                shell.close_cloud_tab(1, cx);
                assert_eq!(shell.cloud_tabs.views.len(), 1);
                let first = shell.cloud_conversation.clone().unwrap();
                assert_eq!(first.read(cx).target.initial_session, "session1");
                let closed = shell.closed_cloud_conversations(cx);
                assert_eq!(
                    closed
                        .iter()
                        .map(|chat| chat.id.as_str())
                        .collect::<Vec<_>>(),
                    vec![session.as_str()]
                );
                shell.restore_cloud_conversation(session.clone(), cx);
                assert_eq!(shell.cloud_tabs.views.len(), 2);
                let front = shell.cloud_conversation.clone().unwrap();
                assert_eq!(front.read(cx).target.initial_session, session);
                assert_eq!(front.read(cx).target.workspace, "task1");
                // Its tab is reused when it is opened again.
                shell.restore_cloud_conversation(session.clone(), cx);
                assert_eq!(shell.cloud_tabs.views.len(), 2);
            })
            .unwrap();
        assert_eq!(creates().len(), 1);
        window
            .update(cx, |_, window, _| window.remove_window())
            .unwrap();
    }
    /// A refused registration shows its reason, keeps sends held, and is
    /// retried with the same request, so the same conversation, until the
    /// server confirms it.
    #[gpui::test]
    fn a_refused_new_cloud_chat_is_retried_as_the_same_conversation(cx: &mut TestAppContext) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _entered = runtime.enter();
        let dir = tempfile::tempdir().unwrap();
        let window = draft_shell(cx, dir.path());
        cx.update(|cx| gpui_tokio::init_from_handle(cx, runtime.handle().clone()));
        let computer = std::sync::Arc::new(Computer {
            refuse: 1.into(),
            ..Default::default()
        });
        let controller =
            crate::state::EngineHandle::from_test_client(zuse_rpc::memory_client(computer.clone()));
        window
            .update(cx, |shell, _, cx| {
                shell.open_cloud_conversation(
                    crate::state::CloudTarget {
                        controller,
                        subject: "owner".into(),
                        organization: None,
                        workspace: "task1".into(),
                        initial_session: "session1".into(),
                    },
                    json!({"workspaceId":"task1","chatId":"thread1","initialSessionId":"session1"}),
                    Value::Null,
                    cx,
                );
                shell.open_new_conversation_tab(cx);
            })
            .unwrap();
        let attempts = || {
            computer
                .seen
                .lock()
                .unwrap()
                .iter()
                .filter(|(method, params)| {
                    method == methods::CLOUD_RPC
                        && params["method"] == methods::CREATE_THREAD_SESSION
                })
                .map(|(_, params)| params.clone())
                .collect::<Vec<_>>()
        };
        let settle = |cx: &mut TestAppContext| {
            for _ in 0..20 {
                cx.run_until_parked();
                runtime.block_on(tokio::task::yield_now());
            }
        };
        settle(cx);
        assert_eq!(attempts().len(), 1);
        window
            .update(cx, |shell, _, cx| {
                let front = shell.cloud_conversation.clone().unwrap();
                assert!(front.read(cx).registering, "sends stay held");
                let composer = front.read(cx).composer.clone();
                composer.update(cx, |composer, cx| {
                    composer.send_for_test("Held".into(), false, cx);
                    assert!(
                        composer
                            .failure
                            .as_ref()
                            .is_some_and(|failure| failure.contains("not confirmed")),
                        "the refusal stays visible"
                    );
                });
            })
            .unwrap();
        cx.executor()
            .advance_clock(std::time::Duration::from_secs(3));
        settle(cx);
        let attempts = attempts();
        assert_eq!(attempts.len(), 2);
        assert_eq!(
            attempts[0], attempts[1],
            "the same request, so the same conversation"
        );
        window
            .update(cx, |shell, _, cx| {
                let front = shell.cloud_conversation.clone().unwrap();
                assert!(!front.read(cx).registering);
                assert!(front.read(cx).composer.read(cx).failure.is_none());
                assert_eq!(shell.cloud_tabs.views.len(), 2);
            })
            .unwrap();
        window
            .update(cx, |_, window, _| window.remove_window())
            .unwrap();
    }
    struct OutletOnly {
        shell: Entity<Shell>,
    }
    impl Render for OutletOnly {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            div().w(px(1200.)).h(px(800.)).child(
                self.shell
                    .update(cx, |shell, cx| shell.render_cloud_launch_outlet(cx)),
            )
        }
    }
    struct ViewOnly {
        view: gpui::AnyView,
    }
    impl Render for ViewOnly {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().w(px(1200.)).h(px(800.)).child(self.view.clone())
        }
    }
    fn sidebar_rows(shell: &Shell, cx: &App) -> Vec<Value> {
        shell
            .cloud_sidebar_groups(cx)
            .into_iter()
            .flat_map(|group| group.rows)
            .collect()
    }
    fn catalog(state: &mut AppState, workspaces: Value) {
        assert!(state.apply_cloud_sidebar_catalog(
            &json!({"accountId":"owner","organizationId":null,"ownerId":"owner","projects":[{"projectId":"p","accountId":"owner","providerId":"boxd","repositoryIdentity":"github.com/owner/p"}]}),
            &json!({"accountId":"owner","organizationId":null,"workspaces":workspaces}),
        ));
    }
    /// The owner's report: New thread → a cloud project → Send. While the
    /// chat is being created its row is in the sidebar and its message sits
    /// at the top, where any send anchors it. The create confirms: the row
    /// stays (no list has it yet, and the list is read again), and the
    /// conversation that takes over anchors the message like any send.
    #[gpui::test]
    fn new_thread_cloud_send_keeps_its_sidebar_row_and_anchors_its_message(
        cx: &mut TestAppContext,
    ) {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let _entered = runtime.enter();
        let dir = tempfile::tempdir().unwrap();
        cx.update(|cx| gpui_tokio::init_from_handle(cx, runtime.handle().clone()));
        let window = draft_shell(cx, dir.path());
        let controller = crate::state::EngineHandle::from_test_client(zuse_rpc::memory_client(
            std::sync::Arc::new(Pending),
        ));
        let older = |id: &str, at: i64| {
            json!({"workspaceId":id,"initialSessionId":format!("{id}-session"),"projectId":"p",
                "title":id,"state":"ready","createdAt":at,"updatedAt":at})
        };
        let listed = json!([
            older("w1", 1),
            older("w2", 2),
            older("w3", 3),
            older("w4", 4)
        ]);
        let first = "yooo so what's cooking?";
        window
            .update(cx, |shell, _, cx| {
                shell.state.update(cx, |state, _| {
                    state.set_test_engine(controller.clone());
                    catalog(state, listed.clone());
                });
                shell.open_new_session(None, cx);
                shell.track_current_cloud_canvas(cx);
                let request = json!({"idempotencyKey":"k","projectId":"p","providerId":"boxd",
                    "firstMessage":first,"agent":"codex","model":"m"});
                shell.composer.update(cx, |composer, _| {
                    composer.seed_pending_cloud_creation_for_test(request)
                });
                let rows = sidebar_rows(shell, cx);
                assert_eq!(rows.len(), 5, "the new chat is listed while it is created");
                assert_eq!(rows[0]["title"], first);
                assert!(rows[0]["pendingLaunch"].is_string());
            })
            .unwrap();
        // The pending launch view: the message at the top, like a sent one.
        let shell = window.entity(cx).unwrap();
        let outlet_host = cx.add_window(|_, _| OutletOnly {
            shell: shell.clone(),
        });
        cx.update_window(outlet_host.into(), |_, window, cx| window.draw(cx).clear())
            .unwrap();
        {
            let mut visual = gpui::VisualTestContext::from_window(outlet_host.into(), cx);
            let prompt = visual
                .debug_bounds("cloud-launch-prompt")
                .expect("the pending view shows the first message");
            let outlet = visual.debug_bounds("cloud-launch-outlet").unwrap();
            let offset = f32::from(prompt.top() - outlet.top());
            assert!(
                (offset - crate::transcript::FIRST_ROW_TOP_GAP_PX).abs() < 1.0,
                "the first message sits where the transcript anchors it, not above the composer (offset {offset})"
            );
        }
        outlet_host
            .update(cx, |_, window, _| window.remove_window())
            .unwrap();
        window
            .update(cx, |shell, _, cx| {
                // The create response admits the chat: the production
                // hand-off from the composer to the shell.
                shell.composer.update(cx, |composer, cx| {
                    composer.admit_pending_cloud_creation_for_test(
                        json!({"workspaceId":"fresh","initialSessionId":"fresh-session",
                            "state":"provisioning","commandDeliveryMode":"server-admitted-v2",
                            "commandAdmissionVersion":2}),
                        cx,
                    )
                });
            })
            .unwrap();
        window
            .update(cx, |shell, _, cx| {
                assert!(shell.composer.read(cx).cloud_launch_summaries().is_empty());
                let rows = sidebar_rows(shell, cx);
                assert_eq!(
                    rows.len(),
                    5,
                    "no gap between the pending and the created row"
                );
                assert_eq!(rows[0]["workspaceId"], "fresh");
                assert_eq!(rows[0]["title"], first);
                assert!(
                    shell.state.read(cx).cloud_sidebar_loading(),
                    "the account list is read again after the create"
                );
                assert!(
                    shell.cloud_conversation.is_some(),
                    "the conversation takes over in the same tab"
                );
            })
            .unwrap();
        // The conversation paints, as the shell paints it in this tab.
        let conversation = window
            .update(cx, |shell, _, _| shell.cloud_conversation.clone().unwrap())
            .unwrap();
        let conversation_host = cx.add_window(|_, _| ViewOnly {
            view: conversation.into(),
        });
        cx.update_window(conversation_host.into(), |_, window, cx| {
            window.draw(cx).clear()
        })
        .unwrap();
        conversation_host
            .update(cx, |_, window, _| window.remove_window())
            .unwrap();
        window
            .update(cx, |shell, _, cx| {
                let view = shell.cloud_conversation.clone().unwrap();
                let transcript = view.read(cx).transcript_for_test().clone();
                assert_eq!(
                    transcript.read(cx).own_send_target().as_deref(),
                    Some("launch-echo:fresh"),
                    "the first message anchors to the top like any send"
                );
                // A list read before the server had it keeps the row.
                shell
                    .state
                    .update(cx, |state, _| catalog(state, listed.clone()));
                assert!(
                    shell
                        .state
                        .read(cx)
                        .cloud_sidebar_workspace("fresh")
                        .is_some(),
                    "a list without the new chat yet does not drop it"
                );
                // The server lists it: its own row replaces the created one.
                let mut with_new = listed.as_array().unwrap().clone();
                with_new.push(
                    json!({"workspaceId":"fresh","initialSessionId":"fresh-session",
                    "projectId":"p","title":first,"state":"ready","createdAt":10,"updatedAt":10}),
                );
                shell
                    .state
                    .update(cx, |state, _| catalog(state, Value::Array(with_new)));
                let rows = sidebar_rows(shell, cx);
                assert_eq!(rows.len(), 5, "one row for the new chat");
                let fresh: Vec<_> = rows
                    .iter()
                    .filter(|row| row["workspaceId"] == "fresh")
                    .collect();
                assert_eq!(fresh.len(), 1);
                assert_eq!(fresh[0]["state"], "ready", "the server's row");
            })
            .unwrap();
        window
            .update(cx, |_, window, _| window.remove_window())
            .unwrap();
    }

    /// A cloud chat whose agent is mid-turn spins in its tab and sidebar row
    /// like a local chat, and stops once the transcript shows the turn done.
    #[gpui::test]
    fn a_working_cloud_chat_spins_in_its_tab_and_sidebar_row(cx: &mut TestAppContext) {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let _entered = runtime.enter();
        let dir = tempfile::tempdir().unwrap();
        cx.update(|cx| gpui_tokio::init_from_handle(cx, runtime.handle().clone()));
        let window = draft_shell(cx, dir.path());
        let controller = crate::state::EngineHandle::from_test_client(zuse_rpc::memory_client(
            std::sync::Arc::new(Pending),
        ));
        let row = json!({"workspaceId":"w","initialSessionId":"s","projectId":"p","title":"Busy",
            "state":"ready","createdAt":1,"updatedAt":1});
        let entry = |id: &str, role, status| zuse_model::SessionMessageEntry {
            id: id.into(),
            role,
            parts: vec![zuse_model::MessagePart::Text {
                id: format!("text:{id}"),
                text: "go".into(),
            }],
            created_at: 1,
            device_id: String::new(),
            status,
            continuation_of: None,
            duration_ms: None,
        };
        let view = window
            .update(cx, |shell, _, cx| {
                shell.state.update(cx, |state, _| {
                    state.set_test_engine(controller.clone());
                    catalog(state, json!([row.clone()]));
                });
                shell.open_cloud_conversation(
                    crate::state::CloudTarget {
                        controller: controller.clone(),
                        subject: "owner".into(),
                        organization: None,
                        workspace: "w".into(),
                        initial_session: "s".into(),
                    },
                    row.clone(),
                    Value::Null,
                    cx,
                );
                let view = shell.cloud_conversation.clone().unwrap();
                view.update(cx, |view, cx| {
                    view.set_admissions_for_test(json!([{"commandId":"c","messageId":"ask",
                        "text":"go","executionState":"running","createdAt":1}]));
                    view.state.update(cx, |state, cx| {
                        state.apply_transcript(vec![
                            entry("ask", zuse_model::MessageRole::User, None),
                            entry(
                                "reply",
                                zuse_model::MessageRole::Assistant,
                                Some(zuse_model::MessageStatus::Streaming),
                            ),
                        ]);
                        cx.notify();
                    });
                });
                view
            })
            .unwrap();
        let shell = window.entity(cx).unwrap();
        shell.read_with(cx, |shell, cx| {
            assert!(
                shell.cloud_workspace_working("w", cx),
                "the sidebar row spins while the agent works"
            );
        });
        let tabs = cx.add_window(|_, _| TabsOnly {
            shell: shell.clone(),
            _dir: tempfile::tempdir().unwrap(),
        });
        cx.update_window(tabs.into(), |_, window, cx| window.draw(cx).clear())
            .unwrap();
        {
            let mut visual = gpui::VisualTestContext::from_window(tabs.into(), cx);
            assert!(
                visual.debug_bounds("cloud-tab-working-0").is_some(),
                "the tab spins while the agent works"
            );
        }
        // The turn completes (its command row may still read running).
        view.update(cx, |view, cx| {
            view.state.update(cx, |state, cx| {
                state.apply_transcript(vec![
                    entry("ask", zuse_model::MessageRole::User, None),
                    entry(
                        "reply",
                        zuse_model::MessageRole::Assistant,
                        Some(zuse_model::MessageStatus::Complete),
                    ),
                ]);
                cx.notify();
            });
        });
        shell.read_with(cx, |shell, cx| {
            assert!(!shell.cloud_workspace_working("w", cx), "done: no spinner");
        });
        cx.update_window(tabs.into(), |_, window, cx| window.draw(cx).clear())
            .unwrap();
        {
            let mut visual = gpui::VisualTestContext::from_window(tabs.into(), cx);
            assert!(visual.debug_bounds("cloud-tab-working-0").is_none());
        }
        tabs.update(cx, |_, window, _| window.remove_window())
            .unwrap();
        window
            .update(cx, |_, window, _| window.remove_window())
            .unwrap();
    }

    /// A file opened from a cloud chat's Files panel gets its own tab beside
    /// the chat tab, as in a local chat: the chat tab switches back to the
    /// conversation, and closing the file tab returns to it too.
    #[gpui::test]
    fn a_cloud_chats_file_opens_in_a_tab_beside_the_chat(cx: &mut TestAppContext) {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let _entered = runtime.enter();
        let dir = tempfile::tempdir().unwrap();
        cx.update(|cx| gpui_tokio::init_from_handle(cx, runtime.handle().clone()));
        let window = draft_shell(cx, dir.path());
        let controller = crate::state::EngineHandle::from_test_client(zuse_rpc::memory_client(
            std::sync::Arc::new(Pending),
        ));
        let row = json!({"workspaceId":"w","initialSessionId":"s","projectId":"p","title":"Cloud",
            "state":"ready","createdAt":1,"updatedAt":1});
        let file = window
            .update(cx, |shell, window, cx| {
                shell.reduced_motion = true;
                shell.state.update(cx, |state, _| {
                    state.set_test_engine(controller.clone());
                    catalog(state, json!([row.clone()]));
                });
                shell.open_cloud_conversation(
                    crate::state::CloudTarget {
                        controller: controller.clone(),
                        subject: "owner".into(),
                        organization: None,
                        workspace: "w".into(),
                        initial_session: "s".into(),
                    },
                    row.clone(),
                    Value::Null,
                    cx,
                );
                assert!(shell.cloud_conversation.is_some());
                shell.add_files_surface(window, cx);
                shell.add_file_surface("README.md".into(), window, cx);
                let file = shell.active_main_tab(cx).expect("the file is in front");
                assert_eq!(shell.main_tab_rows(cx).len(), 1);
                file
            })
            .unwrap();
        let main_tabs::MainTab::File(id) = file else {
            panic!("expected a file tab, got {file:?}");
        };
        let shell = window.entity(cx).unwrap();
        let tabs = cx.add_window(|_, _| TabsOnly {
            shell: shell.clone(),
            _dir: tempfile::tempdir().unwrap(),
        });
        // `debug_bounds` takes a static selector.
        let file_tab: &'static str = format!("main-tab-file-{id}").leak();
        let draw = |cx: &mut TestAppContext| {
            cx.update_window(tabs.into(), |_, window, cx| window.draw(cx).clear())
                .unwrap();
        };
        draw(cx);
        {
            let mut visual = gpui::VisualTestContext::from_window(tabs.into(), cx);
            assert!(
                visual.debug_bounds(file_tab).is_some(),
                "the file has a tab in the cloud strip"
            );
            let chat = visual
                .debug_bounds("cloud-conversation-tab-0")
                .expect("the chat tab stays listed");
            visual.simulate_click(chat.center(), gpui::Modifiers::default());
        }
        shell.read_with(cx, |shell, cx| {
            assert_eq!(
                shell.active_main_tab(cx),
                None,
                "the chat tab shows the chat"
            );
            assert_eq!(shell.main_tab_rows(cx).len(), 1, "the file tab stays");
        });
        draw(cx);
        {
            let mut visual = gpui::VisualTestContext::from_window(tabs.into(), cx);
            let file = visual
                .debug_bounds(file_tab)
                .expect("the file tab is still listed");
            visual.simulate_click(file.center(), gpui::Modifiers::default());
        }
        shell.update(cx, |shell, cx| {
            assert_eq!(shell.active_main_tab(cx), Some(file), "back on the file");
            shell.close_main_tab(file, cx);
            assert_eq!(shell.active_main_tab(cx), None, "closing returns to chat");
            assert!(shell.main_tab_rows(cx).is_empty());
            assert!(shell.cloud_conversation.is_some());
        });
        draw(cx);
        {
            let mut visual = gpui::VisualTestContext::from_window(tabs.into(), cx);
            assert!(visual.debug_bounds(file_tab).is_none());
            assert!(visual.debug_bounds("cloud-conversation-tab-0").is_some());
        }
        tabs.update(cx, |_, window, _| window.remove_window())
            .unwrap();
        // Close the shell before the runtime stops, so no runtime thread
        // wakes a task of it during the shutdown.
        drop(shell);
        window
            .update(cx, |_, window, _| window.remove_window())
            .unwrap();
        cx.run_until_parked();
    }

    /// A sidebar row's cloud is green only while its computer is up: a
    /// sleeping one is muted even while the account list still reads it
    /// running, and an open conversation's live state decides over the list.
    #[gpui::test]
    fn a_sleeping_computers_row_is_not_green(cx: &mut TestAppContext) {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let _entered = runtime.enter();
        let dir = tempfile::tempdir().unwrap();
        cx.update(|cx| gpui_tokio::init_from_handle(cx, runtime.handle().clone()));
        let window = draft_shell(cx, dir.path());
        let controller = crate::state::EngineHandle::from_test_client(zuse_rpc::memory_client(
            std::sync::Arc::new(Pending),
        ));
        let row = |id: &str, state: &str, runtime: &str| {
            json!({"workspaceId":id,"initialSessionId":format!("{id}-s"),"projectId":"p",
                "title":id,"state":state,"runtimeState":runtime,"createdAt":1,"updatedAt":1})
        };
        let view = window
            .update(cx, |shell, _, cx| {
                shell.state.update(cx, |state, _| {
                    state.set_test_engine(controller.clone());
                    catalog(
                        state,
                        json!([
                            row("asleep", "paused", "offline"),
                            row("up", "ready", "online"),
                            row("stale", "ready", "online"),
                        ]),
                    );
                });
                assert!(!shell.cloud_row_online("asleep", cx), "paused: muted");
                assert!(shell.cloud_row_online("up", cx), "ready and online: green");
                // Open, the computer reports going to sleep before any list does.
                shell.open_cloud_conversation(
                    crate::state::CloudTarget {
                        controller: controller.clone(),
                        subject: "owner".into(),
                        organization: None,
                        workspace: "stale".into(),
                        initial_session: "stale-s".into(),
                    },
                    row("stale", "ready", "online"),
                    Value::Null,
                    cx,
                );
                let view = shell.cloud_conversation.clone().unwrap();
                view.update(cx, |view, cx| {
                    view.set_computer_metadata_for_test(
                        json!({"workspaceId":"stale","state":"paused","runtimeState":"offline"}),
                    );
                    cx.notify();
                });
                view
            })
            .unwrap();
        let shell = window.entity(cx).unwrap();
        shell.read_with(cx, |shell, cx| {
            assert!(
                !shell.cloud_row_online("stale", cx),
                "the live state wins over a stale list"
            );
        });
        view.update(cx, |view, cx| {
            view.set_computer_metadata_for_test(
                json!({"workspaceId":"stale","state":"ready","runtimeState":"online"}),
            );
            cx.notify();
        });
        shell.read_with(cx, |shell, cx| assert!(shell.cloud_row_online("stale", cx)));
        window
            .update(cx, |_, window, _| window.remove_window())
            .unwrap();
    }
}
