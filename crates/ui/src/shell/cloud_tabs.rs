//! Explicit cloud tabs retain conversation entities and scoped canvas drafts.
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
    views: Vec<Option<Entity<CloudConversation>>>,
    drafts: Vec<CloudDraft>,
    active: usize,
    scope: Option<(String, Option<String>)>,
}
impl CloudTabs {
    pub(super) fn is_empty(&self) -> bool {
        self.views.is_empty()
    }
    pub(super) fn set_current(&mut self, view: Entity<CloudConversation>, cx: &App) {
        let target = &view.read(cx).target;
        self.scope = Some((target.subject.clone(), target.organization.clone()));
        if self.views.is_empty() {
            self.views.push(None);
            self.drafts.push(CloudDraft::default());
        }
        self.views[self.active] = Some(view);
    }
    pub(super) fn retained_index(
        &self,
        target: &crate::state::CloudTarget,
        cx: &App,
    ) -> Option<usize> {
        self.views.iter().position(|view| {
            view.as_ref().is_some_and(|view| {
                let current = &view.read(cx).target;
                current.subject == target.subject
                    && current.organization == target.organization
                    && current.workspace == target.workspace
                    && current.initial_session == target.initial_session
            })
        })
    }
    pub(super) fn composer_for_session(&self, key: &str, cx: &App) -> Option<Entity<Composer>> {
        self.views.iter().flatten().find_map(|view| {
            let view = view.read(cx);
            (view.target.initial_session == key).then(|| view.composer.clone())
        })
    }
    fn foreign_scope(&self, scope: Option<&(String, Option<String>)>, cx: &App) -> bool {
        self.views.iter().flatten().any(|view| {
            let t = &view.read(cx).target;
            scope != Some(&(t.subject.clone(), t.organization.clone()))
        })
    }
    pub(super) fn clear_current(&mut self) {
        if let Some(slot) = self.views.get_mut(self.active) {
            *slot = None;
        }
    }
}
impl Shell {
    pub(super) fn cloud_tab_navigation_ready(&mut self, cx: &mut Context<Self>) -> bool {
        self.composer.update(cx, |composer, cx| {
            composer.cloud_canvas_navigation_ready(cx)
        })
    }
    fn stash_cloud_canvas(&mut self, cx: &mut Context<Self>) {
        if self.cloud_tabs.active >= self.cloud_tabs.drafts.len()
            || self.cloud_conversation.is_some()
            || self.state.read(cx).selected_chat.is_some()
        {
            return;
        }
        let canvas_key = self
            .composer
            .update(cx, |composer, cx| composer.ensure_cloud_canvas_key(cx));
        if let Some(draft) = self.cloud_tabs.drafts.get_mut(self.cloud_tabs.active) {
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
        if self.cloud_conversation.is_some()
            || self.state.read(cx).selected_chat.is_some()
            || self.state.read(cx).cloud_project.is_none()
        {
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
        if self.cloud_tabs.views.is_empty() {
            self.cloud_tabs.views.push(None);
            self.cloud_tabs.drafts.push(CloudDraft::default());
        }
        self.cloud_tabs.scope = scope;
        self.stash_cloud_canvas(cx);
    }
    pub(super) fn retains_cloud_canvas(&self, key: &str) -> bool {
        self.cloud_tabs.drafts.iter().any(|draft| draft.key == key)
    }
    pub(super) fn initialize_new_cloud_canvas(&mut self, cx: &mut Context<Self>) {
        let state = self.state.read(cx);
        let scope = state.cloud_user_id().map(|id| {
            (
                id.to_owned(),
                state.cloud_organization_id().map(str::to_owned),
            )
        });
        if self.cloud_tabs.views.is_empty() && state.cloud_project.is_none() {
            self.composer
                .update(cx, |composer, cx| composer.set_cloud_canvas_key(None, cx));
            return;
        }
        let project = state.cloud_project.clone();
        let device = state.selected_device.clone();
        let space = state.selected_space.clone();
        if self.cloud_tabs.views.is_empty() {
            self.cloud_tabs.views.push(None);
            self.cloud_tabs.drafts.push(CloudDraft::default());
        }
        self.cloud_tabs.scope = scope;
        let old = std::mem::take(&mut self.cloud_tabs.drafts[self.cloud_tabs.active]);
        self.composer.update(cx, |composer, cx| {
            composer.discard_cloud_canvas_keys(&[old.key], cx)
        });
        let draft = &mut self.cloud_tabs.drafts[self.cloud_tabs.active];
        draft.project = project;
        draft.device = device;
        draft.space = space;
        let key = draft.key.clone();
        let project = draft.project.clone();
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
                .drafts
                .iter()
                .map(|draft| draft.key.clone())
                .collect::<Vec<_>>();
            let active = self.cloud_conversation.is_some();
            self.cloud_conversation_events = None;
            self.cloud_conversation = None;
            self.cloud_tabs = CloudTabs::default();
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
    /// Only explicit plus adds a tab. Pending creation keeps its original surface.
    pub(super) fn open_new_conversation_tab(&mut self, cx: &mut Context<Self>) {
        self.sync_cloud_tab_scope(cx);
        if !self.cloud_tab_navigation_ready(cx) {
            return;
        }
        if self.cloud_conversation.is_none()
            && self.state.read(cx).cloud_project.is_none()
            && self.cloud_tabs.views.is_empty()
        {
            self.open_new_session(None, cx);
            return;
        }
        let home = if let Some(view) = &self.cloud_conversation {
            let project = view.read(cx).project_id().map(str::to_owned);
            let expected = {
                let state = self.state.read(cx);
                state
                    .cloud_organization_id()
                    .map(|id| format!("organization:{id}"))
                    .or_else(|| state.cloud_user_id().map(str::to_owned))
            };
            let row = self
                .state
                .read(cx)
                .cloud_sidebar_projects()
                .into_iter()
                .find(|row| {
                    Some(row["projectId"].as_str().unwrap_or("")) == project.as_deref()
                        && row["accountId"].as_str() == expected.as_deref()
                        && row["canCreate"] != false
                });
            let Some(row) = row else {
                self.composer.update(cx,|composer,cx|{composer.failure=Some("This task's repository is not available for a new cloud tab. Select an authorized repository first.".into());cx.notify();});
                return;
            };
            Some(row)
        } else {
            None
        };
        if self.cloud_tabs.views.is_empty() {
            self.cloud_tabs.views.push(None);
            self.cloud_tabs.drafts.push(CloudDraft::default());
        }
        self.stash_cloud_canvas(cx);
        self.cloud_tabs.views.push(None);
        self.cloud_tabs.drafts.push(CloudDraft::default());
        self.cloud_tabs.active = self.cloud_tabs.views.len() - 1;
        if let Some(project) = home {
            self.open_new_cloud_session(project, cx);
        } else {
            self.open_new_session(None, cx);
        }
    }
    pub(super) fn select_cloud_tab(&mut self, index: usize, cx: &mut Context<Self>) {
        self.sync_cloud_tab_scope(cx);
        if index >= self.cloud_tabs.views.len() || !self.cloud_tab_navigation_ready(cx) {
            return;
        }
        let view = self.cloud_tabs.views[index].clone();
        if let Some(view) = &view {
            let target = &view.read(cx).target;
            let state = self.state.read(cx);
            if state.cloud_user_id() != Some(target.subject.as_str())
                || state.cloud_organization_id() != target.organization.as_deref()
            {
                return;
            }
        }
        self.stash_cloud_canvas(cx);
        self.cloud_tabs.active = index;
        self.cloud_conversation_events = None;
        self.cloud_conversation = view.clone();
        self.state
            .update(cx, |state, cx| state.select_chat(None, cx));
        if let Some(view) = view {
            self.active_chat = view.read(cx).target.initial_session.clone();
            self.subscribe_cloud_conversation(&view, cx);
        } else {
            self.active_chat.clear();
            let draft = &self.cloud_tabs.drafts[index];
            let project = draft.project.clone();
            let choice = draft.choice.clone();
            let key = draft.key.clone();
            let device = draft.device.clone();
            let space = draft.space.clone();
            self.state.update(cx, |state, cx| {
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
            });
        }
        self.set_route(Route::Chat, cx);
        self.focus_composer(cx);
        cx.notify();
    }
    fn close_cloud_tab(&mut self, index: usize, cx: &mut Context<Self>) {
        self.sync_cloud_tab_scope(cx);
        if index >= self.cloud_tabs.views.len() || !self.cloud_tab_navigation_ready(cx) {
            return;
        }
        let active = index == self.cloud_tabs.active;
        if active {
            self.stash_cloud_canvas(cx);
        }
        let draft = self.cloud_tabs.drafts.remove(index);
        self.cloud_tabs.views.remove(index);
        self.composer.update(cx, |composer, cx| {
            composer.discard_cloud_canvas_keys(&[draft.key], cx)
        });
        if self.cloud_tabs.views.is_empty() {
            self.cloud_tabs.active = 0;
            self.open_new_session(None, cx);
        } else if active {
            self.cloud_tabs.active = usize::MAX;
            self.select_cloud_tab(index.min(self.cloud_tabs.views.len() - 1), cx);
        } else if index < self.cloud_tabs.active {
            self.cloud_tabs.active -= 1;
        }
        cx.notify();
    }
    pub(super) fn render_cloud_tabs(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        self.sync_cloud_tab_scope(cx);
        if !matches!(self.route, Route::Chat)
            || self.cloud_tabs.views.is_empty()
            || (self.cloud_conversation.is_none() && self.state.read(cx).selected_chat.is_some())
        {
            return None;
        }
        let theme = Theme::of(cx).clone();
        let tabs = self
            .cloud_tabs
            .views
            .iter()
            .enumerate()
            .map(|(index, view)| {
                let title = view
                    .as_ref()
                    .map(|view| view.read(cx).title.clone())
                    .unwrap_or_else(|| "New chat".into());
                let active = index == self.cloud_tabs.active;
                div()
                    .id(("cloud-conversation-tab", index))
                    .flex()
                    .items_center()
                    .min_w_0()
                    .max_w(px(220.))
                    .px(px(8.))
                    .py(px(5.))
                    .gap(px(8.))
                    .rounded_md()
                    .when(active, |tab| tab.bg(theme.muted_fill()))
                    .cursor_pointer()
                    .text_size(px(12.))
                    .child(div().min_w_0().truncate().child(title))
                    .child(
                        div()
                            .id(("close-cloud-tab", index))
                            .cursor_pointer()
                            .child(icon(icons::CLOSE).size(px(12.)))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.close_cloud_tab(index, cx);
                            })),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| this.select_cloud_tab(index, cx)))
            })
            .collect::<Vec<_>>();
        Some(
            div()
                .flex_none()
                .flex()
                .items_center()
                .gap(px(4.))
                .px(px(8.))
                .py(px(4.))
                .children(tabs)
                .child(
                    div()
                        .id("cloud-new-tab")
                        .debug_selector(|| "cloud-new-tab".into())
                        .role(gpui::Role::Button)
                        .aria_label("New cloud tab")
                        .cursor_pointer()
                        .p(px(5.))
                        .child(icon(icons::PLUS).size(px(14.)))
                        .on_click(cx.listener(|this, _, _, cx| this.open_new_conversation_tab(cx))),
                )
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{AppContext, TestAppContext};
    use serde_json::{Value, json};
    struct Pending;
    #[async_trait::async_trait]
    impl zuse_rpc::RpcService for Pending {
        async fn handle(
            &self,
            _: &str,
            _: Value,
        ) -> Result<zuse_rpc::RpcReply, zuse_rpc::RpcError> {
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
    fn blank_tabs_keep_distinct_full_drafts_projects_models_and_clear_on_scope_change(
        cx: &mut TestAppContext,
    ) {
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
                shell.select_cloud_tab(0, cx);
                let c = shell.composer.read(cx);
                assert_eq!(c.current_key, first_key);
                assert_eq!(c.input.read(cx).text(), "First unsent draft");
                assert_eq!(c.attachments[&first_key][0].name, "first.txt");
                assert_eq!(c.appshots[&first_key].len(), 1);
                assert_eq!(
                    shell.state.read(cx).cloud_project.as_ref().unwrap()["projectId"],
                    "p"
                );
                assert_eq!(
                    c.pickers().read(cx).resolved(cx).model.as_deref(),
                    Some("boxd-model")
                );
                assert_eq!(
                    c.pickers().read(cx).source_cloud_ref("main", cx).0,
                    "refs/heads/first"
                );
                shell.select_cloud_tab(1, cx);
                let c = shell.composer.read(cx);
                assert_eq!(c.current_key, second_key);
                assert_eq!(c.input.read(cx).text(), "Second unsent draft");
                assert_eq!(c.attachments[&second_key][0].name, "second.txt");
                assert_eq!(
                    shell.state.read(cx).cloud_project.as_ref().unwrap()["projectId"],
                    "q"
                );
                assert_eq!(
                    c.pickers().read(cx).resolved(cx).model.as_deref(),
                    Some("e2b-model")
                );
                assert_eq!(
                    c.pickers().read(cx).source_cloud_ref("main", cx).0,
                    "refs/heads/second"
                );
                shell.composer.update(cx, |c, cx| {
                    c.pickers().update(cx, |p, cx| {
                        p.seed_cloud_source_for_test(
                            zuse_protocol::creation_source::Resolution::Issue {
                                number: 23,
                                title: "Issue in second tab".into(),
                                attachment_path: "issue.md".into(),
                                markdown: "Scoped issue body".into(),
                                upload_id: None,
                            },
                            cx,
                        )
                    })
                });
                shell.select_cloud_tab(0, cx);
                assert_eq!(
                    shell
                        .composer
                        .read(cx)
                        .pickers()
                        .read(cx)
                        .source_cloud_ref("main", cx)
                        .0,
                    "refs/heads/first"
                );
                shell.select_cloud_tab(1, cx);
                assert_eq!(
                    shell
                        .composer
                        .read(cx)
                        .pickers()
                        .read(cx)
                        .source_issue(cx)
                        .unwrap()
                        .0,
                    23
                );
                assert_eq!(
                    shell.composer.read(cx).input.read(cx).text(),
                    "Second unsent draft",
                    "restoring an issue must not replay its user-selection title into the draft"
                );
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
    #[gpui::test]
    fn pending_creation_refuses_plus_new_chat_and_project_navigation_without_losing_receipt(
        cx: &mut TestAppContext,
    ) {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let _entered = runtime.enter();
        let dir = tempfile::tempdir().unwrap();
        let window = draft_shell(cx, dir.path());
        window.update(cx,|shell,_,cx| {
            shell.open_new_session(None,cx);let key=shell.composer.read(cx).current_key.clone();let project=shell.state.read(cx).cloud_project.clone();let request=json!({"idempotencyKey":"one-original-key","projectId":"p","providerId":"e2b","firstMessage":"Keep this task"});
            shell.composer.update(cx,|composer,cx|{composer.input.update(cx,|input,cx|input.set_text("Retained text",cx));composer.seed_pending_cloud_creation_for_test(request.clone());});
            shell.open_new_conversation_tab(cx);shell.open_new_session(None,cx);shell.open_new_cloud_session(json!({"projectId":"q","accountId":"owner"}),cx);
            assert_eq!(shell.cloud_tabs.views.len(),1);assert_eq!(shell.cloud_tabs.active,0);assert_eq!(shell.state.read(cx).cloud_project,project);let c=shell.composer.read(cx);assert_eq!(c.current_key,key);assert_eq!(c.pending_cloud_creation_for_test(),Some(&request));assert_eq!(c.input.read(cx).text(),"Retained text");assert!(c.failure.as_ref().unwrap().contains("original request"));
            shell.state.update(cx,|state,_|{state.apply_cloud_account(json!({"user":{"id":"other"},"orgId":null}));});shell.sync_cloud_tab_scope(cx);assert!(shell.composer.read(cx).pending_cloud_creation_for_test().is_none(),"scope revocation retires the prior account's authority instead of retaining it under another account");
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
                assert_eq!(shell.cloud_tabs.drafts[0].key, key);
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
                shell.open_new_conversation_tab(cx);
                let second = shell.composer.read(cx).current_key.clone();
                assert_ne!(first, second);
                assert!(shell.receive_scoped_appshot(
                    identity.clone(),
                    crate::appshots::tests::shot(),
                    window,
                    cx
                ));
                assert_eq!(shell.composer.read(cx).appshots[&first].len(), 1);
                assert!(!shell.composer.read(cx).appshots.contains_key(&second));
                shell.close_cloud_tab(0, cx);
                assert!(!shell.receive_scoped_appshot(
                    identity,
                    crate::appshots::tests::shot(),
                    window,
                    cx
                ));
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
        shell.update(cx, |shell, cx| {
            shell.settings.sidebar_collapsed = false;
            shell.open_new_session(None, cx);
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
        shell.read_with(cx, |shell, _| assert_eq!(shell.cloud_tabs.views.len(), 2));
        handle
            .update(cx, |_, window, _| window.remove_window())
            .unwrap();
        shell_window
            .update(cx, |_, window, _| window.remove_window())
            .unwrap();
    }
    #[gpui::test]
    fn plus_retains_conversation_new_chat_replaces_current_tab_and_scope_change_clears_titles(
        cx: &mut TestAppContext,
    ) {
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
                    json!({"workspaceId":"task1","projectId":"p"}),
                    json!({"firstMessage":"First task"}),
                    cx,
                );
                let first = shell.cloud_conversation.clone().unwrap();
                let state = first.read(cx).state.entity_id();
                shell.state.update(cx,|state,_| {state.spaces.push(serde_json::from_value(json!({"id":"old-local","deviceId":"local","path":"/local/unrelated","createdAt":"2026-10-31T00:00:00Z"})).unwrap());state.selected_space=Some("old-local".into());});
                shell.open_new_conversation_tab(cx);
                assert_eq!(shell.state.read(cx).cloud_project.as_ref().unwrap()["projectId"],"p","plus from a bound task homes to its authorized cloud repository despite stale local selection");
                assert_eq!(shell.cloud_tabs.views.len(), 2);
                assert!(shell.cloud_conversation.is_none());
                assert_eq!(
                    shell.cloud_tabs.views[0].as_ref().unwrap().entity_id(),
                    first.entity_id()
                );
                assert!(shell.render_cloud_tabs(cx).is_some());
                shell.state.update(cx, |state, _| {
                    state.selected_chat = Some("local-selection".into())
                });
                shell.select_cloud_tab(0, cx);
                assert!(
                    shell.state.read(cx).selected_chat.is_none(),
                    "retained cloud restoration clears local selection synchronously"
                );
                assert_eq!(
                    shell.conversation_state(cx).entity_id(),
                    state,
                    "switching reuses the live state and transcript"
                );
                shell.open_new_session(None, cx);
                assert_eq!(
                    shell.cloud_tabs.views.len(),
                    2,
                    "New Chat must not add a tab"
                );
                assert!(shell.cloud_tabs.views[0].is_none());
                assert!(shell.cloud_conversation.is_none());
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
}
