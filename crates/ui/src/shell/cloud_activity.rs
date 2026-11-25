//! Cloud chats' live status, whichever chat is open. Every open cloud
//! conversation (in front, in a tab, or left running in the background)
//! reports its agent's activity to the app state, the one place sidebar rows
//! and tabs read a chat's status from. A conversation whose agent is still
//! working, or waiting on the user, stays alive after the user moves to
//! another chat, so its row keeps spinning and shows "done" when the turn
//! ends instead of falling back to the computer's state.
use super::cloud_conversation::CloudConversation;
use super::*;
use gpui::EntityId;
use std::collections::HashMap;

#[derive(Default)]
pub(super) struct CloudActivity {
    /// Each tracked conversation's workspace and its change and release
    /// watches.
    watches: HashMap<EntityId, (String, [Subscription; 2])>,
    /// Conversations whose agent is busy, kept alive until the turn ends.
    busy: HashMap<EntityId, Entity<CloudConversation>>,
    /// Withdraws every report when the shell closes: its conversations'
    /// release watches no longer run then.
    teardown: Option<Subscription>,
}
impl CloudActivity {
    /// The account changed: another account's chats stop reporting.
    pub(super) fn clear(&mut self) {
        self.busy.clear();
    }
}
impl Shell {
    /// Report `view`'s activity now and on every change until it closes.
    pub(super) fn track_cloud_conversation(
        &mut self,
        view: &Entity<CloudConversation>,
        cx: &mut Context<Self>,
    ) {
        let id = view.entity_id();
        if self.cloud_activity.watches.contains_key(&id) {
            return;
        }
        let mut title = view.read(cx).tab_title(cx);
        let changes = cx.observe(view, move |this, view, cx| {
            let next_title = view.read(cx).tab_title(cx);
            if next_title != title {
                title = next_title;
                cx.notify();
            }
            this.report_cloud_activity(&view, cx);
        });
        let release = cx.observe_release(view, move |this, view, cx| {
            this.cloud_activity.watches.remove(&id);
            if !this.cloud_scope_matches(&view.target, cx) {
                return;
            }
            let workspace = view.target.workspace.clone();
            this.state.update(cx, |state, cx| {
                if state.report_cloud_chat_activity(&workspace, id.as_u64(), None, false) {
                    cx.notify();
                }
            });
        });
        let workspace = view.read(cx).target.workspace.clone();
        self.cloud_activity
            .watches
            .insert(id, (workspace, [changes, release]));
        if self.cloud_activity.teardown.is_none() {
            self.cloud_activity.teardown = Some(cx.on_release(|this, cx| {
                let watches = std::mem::take(&mut this.cloud_activity.watches);
                this.state.update(cx, |state, cx| {
                    let mut changed = false;
                    for (id, (workspace, _)) in watches {
                        changed |=
                            state.report_cloud_chat_activity(&workspace, id.as_u64(), None, true);
                    }
                    if changed {
                        cx.notify();
                    }
                });
            }));
        }
        self.report_cloud_activity(view, cx);
    }
    fn report_cloud_activity(&mut self, view: &Entity<CloudConversation>, cx: &mut Context<Self>) {
        let id = view.entity_id();
        let (workspace, report, in_scope) = {
            let conversation = view.read(cx);
            (
                conversation.target.workspace.clone(),
                conversation.live_activity(cx),
                self.cloud_scope_matches(&conversation.target, cx),
            )
        };
        if in_scope && (report.working || report.awaiting_input) {
            self.cloud_activity
                .busy
                .entry(id)
                .or_insert_with(|| view.clone());
        } else {
            // Released once no tab or front view holds it either.
            self.cloud_activity.busy.remove(&id);
        }
        if !in_scope {
            return;
        }
        // Only a conversation on screen sees its turn end.
        let in_front = matches!(self.route, Route::Chat)
            && self
                .cloud_conversation
                .as_ref()
                .is_some_and(|front| front.entity_id() == id);
        self.state.update(cx, |state, cx| {
            if state.report_cloud_chat_activity(&workspace, id.as_u64(), Some(report), in_front) {
                cx.notify();
            }
        });
    }
    /// `view` came to the front: its chat's finished turn is seen.
    pub(super) fn mark_cloud_conversation_seen(
        &mut self,
        view: &Entity<CloudConversation>,
        cx: &mut Context<Self>,
    ) {
        let workspace = view.read(cx).target.workspace.clone();
        self.state.update(cx, |state, cx| {
            if state.mark_cloud_chat_seen(&workspace) {
                cx.notify();
            }
        });
    }
    /// A conversation for `target` still running in the background, to
    /// bring back instead of opening a second one.
    pub(super) fn background_cloud_conversation(
        &self,
        target: &crate::state::CloudTarget,
        cx: &App,
    ) -> Option<Entity<CloudConversation>> {
        self.cloud_activity
            .busy
            .values()
            .find(|view| {
                let current = &view.read(cx).target;
                current.subject == target.subject
                    && current.organization == target.organization
                    && current.workspace == target.workspace
                    && current.initial_session == target.initial_session
            })
            .cloned()
    }
    fn cloud_scope_matches(&self, target: &crate::state::CloudTarget, cx: &App) -> bool {
        let state = self.state.read(cx);
        state.cloud_user_id() == Some(target.subject.as_str())
            && state.cloud_organization_id() == target.organization.as_deref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{AppContext, TestAppContext};
    use serde_json::{Value, json};
    use zuse_protocol::ChatIndicator;

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
    fn row(id: &str) -> Value {
        json!({"workspaceId":id,"initialSessionId":format!("{id}-s"),"projectId":"p",
            "title":id,"state":"ready","createdAt":1,"updatedAt":1})
    }
    fn shell(cx: &mut TestAppContext, dir: &std::path::Path) -> gpui::WindowHandle<Shell> {
        cx.update(|cx| {
            gpui_base::init(cx);
            cx.set_global(Theme::default());
            crate::app_menus::init(cx);
        });
        let controller = crate::state::EngineHandle::from_test_client(zuse_rpc::memory_client(
            std::sync::Arc::new(Pending),
        ));
        cx.add_window(|_, cx| {
            let state = cx.new(|_| {
                let mut state = AppState::new();
                state.apply_cloud_account(json!({"user":{"id":"owner"},"orgId":null}));
                state.set_test_engine(controller);
                state.apply_cloud_sidebar_catalog(
                    &json!({"accountId":"owner","organizationId":null,"ownerId":"owner",
                        "projects":[{"projectId":"p","accountId":"owner","providerId":"boxd",
                            "repositoryIdentity":"github.com/owner/p"}]}),
                    &json!({"accountId":"owner","organizationId":null,
                        "workspaces":[row("a"), row("b")]}),
                );
                state
            });
            Shell::new(
                state,
                EngineBootConfig {
                    target: None,
                    data_dir: dir.into(),
                    ipc_port: 0,
                    edge_url: "http://127.0.0.1:1".into(),
                    edge_token: None,
                    org_id: None,
                    workos_client_id: None,
                    default_harness: zuse_protocol::HarnessId::Mock,
                },
                cx,
            )
        })
    }
    /// Close the shell before the runtime stops, so no runtime thread
    /// wakes a task of it during the shutdown.
    fn close(window: gpui::WindowHandle<Shell>, shell: Entity<Shell>, cx: &mut TestAppContext) {
        drop(shell);
        window
            .update(cx, |_, window, _| window.remove_window())
            .unwrap();
        cx.run_until_parked();
    }
    fn entry(
        id: &str,
        role: zuse_model::MessageRole,
        status: Option<zuse_model::MessageStatus>,
    ) -> zuse_model::SessionMessageEntry {
        zuse_model::SessionMessageEntry {
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
        }
    }
    /// `view`'s agent has a turn in `execution` with the reply `reply`
    /// (`None`: no reply yet).
    fn turn(
        view: &Entity<CloudConversation>,
        execution: &str,
        reply: Option<zuse_model::MessageStatus>,
        cx: &mut TestAppContext,
    ) {
        view.update(cx, |view, cx| {
            view.set_admissions_for_test(json!([{"commandId":"c","messageId":"ask",
                "text":"go","executionState":execution,"createdAt":1}]));
            view.state.update(cx, |state, cx| {
                let mut entries = vec![entry("ask", zuse_model::MessageRole::User, None)];
                if let Some(status) = reply {
                    entries.push(entry(
                        "reply",
                        zuse_model::MessageRole::Assistant,
                        Some(status),
                    ));
                }
                state.apply_transcript(entries);
                cx.notify();
            });
        });
    }
    fn working(view: &Entity<CloudConversation>, cx: &mut TestAppContext) {
        turn(
            view,
            "running",
            Some(zuse_model::MessageStatus::Streaming),
            cx,
        );
    }
    fn finished(view: &Entity<CloudConversation>, cx: &mut TestAppContext) {
        turn(
            view,
            "running",
            Some(zuse_model::MessageStatus::Complete),
            cx,
        );
    }
    fn status(shell: &Entity<Shell>, workspace: &str, cx: &mut TestAppContext) -> ChatIndicator {
        shell.read_with(cx, |shell, cx| shell.cloud_row_status(&row(workspace), cx))
    }
    fn front(shell: &Entity<Shell>, cx: &mut TestAppContext) -> Option<String> {
        shell.read_with(cx, |shell, cx| {
            shell
                .cloud_conversation
                .as_ref()
                .map(|view| view.read(cx).target.workspace.clone())
        })
    }
    fn open(shell: &Entity<Shell>, workspace: &str, cx: &mut TestAppContext) {
        shell.update(cx, |shell, cx| {
            shell.open_cloud_sidebar_row(&format!("{CLOUD_ROW_PREFIX}{workspace}"), cx)
        });
        assert_eq!(front(shell, cx).as_deref(), Some(workspace));
    }

    /// The owner's report: a cloud chat's row spun only while it was open.
    /// Opening another chat from the sidebar must not stop it: the row keeps
    /// spinning until the agent finishes, then reads done until the chat is
    /// opened again.
    #[gpui::test]
    fn a_cloud_chat_left_working_keeps_its_row_live_and_settles_done(cx: &mut TestAppContext) {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let _entered = runtime.enter();
        let dir = tempfile::tempdir().unwrap();
        cx.update(|cx| gpui_tokio::init_from_handle(cx, runtime.handle().clone()));
        let window = shell(cx, dir.path());
        let shell = window.entity(cx).unwrap();
        open(&shell, "a", cx);
        let a = shell.read_with(cx, |shell, _| shell.cloud_conversation.clone().unwrap());
        working(&a, cx);
        assert_eq!(status(&shell, "a", cx), ChatIndicator::Working);

        // Another chat comes to the front, in A's place.
        open(&shell, "b", cx);
        let a = a.downgrade();
        assert_eq!(
            status(&shell, "a", cx),
            ChatIndicator::Working,
            "A keeps spinning while B is open"
        );
        assert_eq!(status(&shell, "b", cx), ChatIndicator::Idle);
        let target = shell.read_with(cx, |shell, cx| crate::state::CloudTarget {
            controller: shell.state.read(cx).engine().unwrap().controller(),
            subject: "owner".into(),
            organization: None,
            workspace: "a".into(),
            initial_session: "a-s".into(),
        });
        let background = shell
            .read_with(cx, |shell, cx| {
                shell.background_cloud_conversation(&target, cx)
            })
            .expect("A's conversation keeps running in the background");
        assert_eq!(Some(background.clone()), a.upgrade());

        // A finishes while B is open: done, not idle, and let go.
        finished(&background, cx);
        drop(background);
        cx.run_until_parked();
        assert_eq!(status(&shell, "a", cx), ChatIndicator::Completed);
        assert_eq!(status(&shell, "b", cx), ChatIndicator::Idle);
        assert!(
            shell
                .read_with(cx, |shell, cx| shell
                    .background_cloud_conversation(&target, cx))
                .is_none(),
            "a finished background conversation is let go"
        );

        // Opening A again shows the finished turn: seen.
        open(&shell, "a", cx);
        assert_eq!(status(&shell, "a", cx), ChatIndicator::Idle);
        close(window, shell, cx);
    }

    /// Reopening a chat whose conversation still runs in the background
    /// brings that conversation back instead of opening a second one.
    #[gpui::test]
    fn reopening_a_working_cloud_chat_brings_back_its_conversation(cx: &mut TestAppContext) {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let _entered = runtime.enter();
        let dir = tempfile::tempdir().unwrap();
        cx.update(|cx| gpui_tokio::init_from_handle(cx, runtime.handle().clone()));
        let window = shell(cx, dir.path());
        let shell = window.entity(cx).unwrap();
        open(&shell, "a", cx);
        let a = shell.read_with(cx, |shell, _| shell.cloud_conversation.clone().unwrap());
        working(&a, cx);
        open(&shell, "b", cx);
        open(&shell, "a", cx);
        let reopened = shell.read_with(cx, |shell, _| shell.cloud_conversation.clone().unwrap());
        assert_eq!(reopened, a, "the same conversation, still working");
        assert_eq!(status(&shell, "a", cx), ChatIndicator::Working);
        close(window, shell, cx);
    }

    /// A cloud chat kept in its tab while another chat is in front still
    /// reports: waiting on input asks for it, and a finished turn reads done
    /// until the chat comes back to the front.
    #[gpui::test]
    fn a_cloud_chat_in_a_background_tab_reports_input_and_done(cx: &mut TestAppContext) {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let _entered = runtime.enter();
        let dir = tempfile::tempdir().unwrap();
        cx.update(|cx| gpui_tokio::init_from_handle(cx, runtime.handle().clone()));
        let window = shell(cx, dir.path());
        let shell = window.entity(cx).unwrap();
        open(&shell, "a", cx);
        let a = shell.read_with(cx, |shell, _| shell.cloud_conversation.clone().unwrap());
        // A local chat comes to the front; A stays in its tab.
        shell.update(cx, |shell, cx| shell.open_chat("local".into(), cx));
        assert_eq!(front(&shell, cx), None);
        turn(&a, "waiting-input", None, cx);
        assert_eq!(status(&shell, "a", cx), ChatIndicator::AwaitingInput);
        working(&a, cx);
        assert_eq!(status(&shell, "a", cx), ChatIndicator::Working);
        finished(&a, cx);
        assert_eq!(status(&shell, "a", cx), ChatIndicator::Completed);
        // Back to A's tab: seen.
        open(&shell, "a", cx);
        assert_eq!(status(&shell, "a", cx), ChatIndicator::Idle);
        close(window, shell, cx);
    }

    /// A chat left in front behind another page (the plugin catalog) is not
    /// on screen: its turn ending reads done until the chat shows again.
    #[gpui::test]
    fn a_turn_ending_behind_another_page_reads_done_until_the_chat_shows(cx: &mut TestAppContext) {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let _entered = runtime.enter();
        let dir = tempfile::tempdir().unwrap();
        cx.update(|cx| gpui_tokio::init_from_handle(cx, runtime.handle().clone()));
        let window = shell(cx, dir.path());
        let shell = window.entity(cx).unwrap();
        open(&shell, "a", cx);
        let a = shell.read_with(cx, |shell, _| shell.cloud_conversation.clone().unwrap());
        working(&a, cx);
        shell.update(cx, |shell, cx| shell.set_route(Route::Plugins, cx));
        finished(&a, cx);
        assert_eq!(status(&shell, "a", cx), ChatIndicator::Completed);
        shell.update(cx, |shell, cx| shell.set_route(Route::Chat, cx));
        assert_eq!(status(&shell, "a", cx), ChatIndicator::Idle);
        close(window, shell, cx);
    }

    /// Closing the shell withdraws what its conversations reported, so a
    /// chat that was working then does not spin forever.
    #[gpui::test]
    fn closing_the_shell_withdraws_its_reports(cx: &mut TestAppContext) {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let _entered = runtime.enter();
        let dir = tempfile::tempdir().unwrap();
        cx.update(|cx| gpui_tokio::init_from_handle(cx, runtime.handle().clone()));
        let window = shell(cx, dir.path());
        let shell = window.entity(cx).unwrap();
        let state = shell.read_with(cx, |shell, _| shell.state.clone());
        open(&shell, "a", cx);
        let a = shell.read_with(cx, |shell, _| shell.cloud_conversation.clone().unwrap());
        working(&a, cx);
        drop(a);
        assert!(state.read_with(cx, |state, _| state.cloud_chat_activity("a").live.is_some()));
        close(window, shell, cx);
        assert_eq!(
            state.read_with(cx, |state, _| state.cloud_chat_activity("a")),
            Default::default()
        );
    }

    /// A turn that finishes while its chat is in front is seen as it ends;
    /// one that finishes out of sight stays unseen after its view closes.
    #[test]
    fn a_turn_is_unseen_only_when_it_finishes_out_of_sight() {
        use crate::state::CloudLiveActivity;
        let mut state = AppState::new();
        let working = CloudLiveActivity {
            working: true,
            ..Default::default()
        };
        assert!(state.report_cloud_chat_activity("w", 1, Some(working), true));
        assert!(state.report_cloud_chat_activity("w", 1, Some(Default::default()), true));
        assert!(!state.cloud_chat_activity("w").unseen);
        state.report_cloud_chat_activity("w", 1, Some(working), false);
        state.report_cloud_chat_activity("w", 1, Some(Default::default()), false);
        state.report_cloud_chat_activity("w", 1, None, false);
        let activity = state.cloud_chat_activity("w");
        assert!(activity.unseen && activity.live.is_none());
        assert!(state.mark_cloud_chat_seen("w"));
        assert_eq!(state.cloud_chat_activity("w"), Default::default());
        // Two conversations on one computer: working while either is.
        state.report_cloud_chat_activity("w", 1, Some(working), false);
        state.report_cloud_chat_activity("w", 2, Some(Default::default()), false);
        assert!(state.cloud_chat_activity("w").live.unwrap().working);
    }
}
