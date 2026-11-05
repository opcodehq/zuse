//! Opening a cloud draft is independent of a local checkout or running guest.
use super::*;
pub(super) fn account_control_method(method: &str) -> bool {
    matches!(
        method,
        methods::CLOUD_ACCOUNT_STATUS
            | methods::CLOUD_CONTEXT
            | methods::CLOUD_WORKSPACES
            | methods::CLOUD_EVENTS
            | methods::CLOUD_BILLING_ENTITLEMENTS
            | methods::CLOUD_BILLING_CHECKOUT
            | methods::CLOUD_ACCOUNT_SIGN_IN
            | methods::CLOUD_ACCOUNT_SIGN_OUT
    )
}
impl WorkspaceView {
    pub(crate) fn forward_cloud_account_control(&mut self, request: &Value) -> bool {
        let Some(method) = request["method"]
            .as_str()
            .filter(|method| account_control_method(method))
        else {
            return false;
        };
        let Some(child) = request["id"].as_u64() else {
            return false;
        };
        let mut params = request["params"].clone();
        if !params.is_object() {
            params = json!({});
        }
        params["expectedAccountId"] = json!(self.cloud.account);
        params["expectedOrganizationId"] = json!(self.cloud.organization);
        let id = self.call(method, params);
        self.cloud.forwards.insert(id, (child, method.to_owned()));
        true
    }
    // Account authority belongs to the controller, never to the task guest.
    pub(crate) fn seed_cloud_child_authority(&self, view: &mut Self) {
        view.cloud_account = self.cloud_account.clone();
        view.cloud.account = self.cloud.account.clone();
        view.cloud.organization = self.cloud.organization.clone();
        view.cloud.namespace = self.cloud.namespace.clone();
        view.cloud.projects = self.cloud.projects.clone();
        view.cloud.workspaces = self.cloud.workspaces.clone();
        view.cloud.provider_catalogs = self.cloud.provider_catalogs.clone();
        view.cloud.capabilities = self.cloud.capabilities.clone();
        view.cloud.connections = self.cloud.connections.clone();
        view.cloud.connection = self.cloud.connection.clone();
        view.cloud.authority = self.cloud.authority;
        view.cloud.command_api_version = self.cloud.command_api_version;
        view.cloud.metadata_thread = self.cloud.target.as_ref().and_then(|target| {
            let thread = target["chatId"].as_str().filter(|id| !id.is_empty())?;
            let initial = target["initialSessionId"]
                .as_str()
                .filter(|id| !id.is_empty())?;
            (thread != initial).then(|| thread.to_owned())
        });
        view.cloud.admitted_title = self
            .cloud
            .head
            .as_ref()
            .and_then(|head| {
                head.chat
                    .title
                    .clone()
                    .filter(|title| !title.trim().is_empty())
                    .map(|title| (head.session_id.clone(), title))
            })
            .or_else(|| {
                let target = self.cloud.target.as_ref()?;
                Some((
                    target["initialSessionId"].as_str()?.to_owned(),
                    target["title"]
                        .as_str()
                        .filter(|title| !title.trim().is_empty())?
                        .to_owned(),
                ))
            });
    }
    pub(crate) fn initialize_live_cloud_child(&self, view: &mut Self) {
        self.seed_cloud_child_authority(view);
        view.cloud.guest = true;
        view.cloud.guest_status = self.cloud_guest_status();
        view.cloud.guest_workspace = self
            .cloud
            .target
            .as_ref()
            .and_then(|target| target["workspaceId"].as_str())
            .map(str::to_owned);
        view.preferences = self.preferences.clone();
        view.cloud.initial_session = self
            .cloud
            .head
            .as_ref()
            .map(|head| head.session_id.clone())
            .or_else(|| {
                self.cloud
                    .target
                    .as_ref()
                    .and_then(|target| target["initialSessionId"].as_str())
                    .map(str::to_owned)
            });
    }

    pub(crate) fn receive_cloud_conversations(&self, value: &Value) -> Vec<Value> {
        let mut chats = value.as_array().cloned().unwrap_or_default();
        if !self.cloud.guest {
            return chats;
        }
        // A distinct task thread is a container, not an agent conversation.
        // Keep it in the runtime for session creation, outside selectable chats.
        if let Some(thread) = &self.cloud.metadata_thread {
            chats.retain(|chat| chat["id"].as_str() != Some(thread.as_str()));
        }
        if let Some((session, title)) = &self.cloud.admitted_title {
            if let Some(chat) = chats
                .iter_mut()
                .find(|chat| chat["id"].as_str() == Some(session.as_str()))
            {
                if !chat["title"]
                    .as_str()
                    .is_some_and(|title| !title.trim().is_empty())
                {
                    chat["title"] = json!(title);
                }
            }
        }
        chats
    }
    pub(crate) fn cloud_account_api_available(&self) -> bool {
        self.cloud.namespace.is_some()
            && self
                .cloud
                .account
                .as_deref()
                .is_some_and(|owner| self.cloud_account["user"]["id"].as_str() == Some(owner))
            && self.cloud_account["orgId"].as_str() == self.cloud.organization.as_deref()
    }
    pub(crate) fn sync_cloud_navigation_catalog(&self, cx: &mut Context<Self>) {
        if let Some(child) = &self.cloud.child {
            child.update(cx, |view, cx| {
                self.seed_cloud_child_authority(view);
                cx.notify();
            });
        }
    }
    pub(crate) fn remember_cloud_admitted_workspace(&mut self, row: &mut Value) {
        let Some(id) = row["workspaceId"]
            .as_str()
            .filter(|id| !id.is_empty())
            .map(str::to_owned)
        else {
            return;
        };
        if let Some(request) = &self.cloud.accepted {
            for key in ["projectId", "providerId"] {
                if !row[key].is_string() {
                    row[key] = request[key].clone();
                }
            }
            if !row["title"].as_str().is_some_and(|value| !value.is_empty()) {
                row["title"] = json!(
                    request["firstMessage"]
                        .as_str()
                        .unwrap_or("New cloud chat")
                        .chars()
                        .take(70)
                        .collect::<String>()
                );
            }
        }
        // A pre-admission list response cannot erase this committed task.
        if let Some(id) = self.cloud.list.take() {
            self.cancel(id);
            self.pending.remove(&id);
        }
        self.cloud.workspaces.retain(|row| row["workspaceId"] != id);
        self.cloud.workspaces.insert(0, row.clone());
        debug_assert_eq!(
            self.cloud
                .workspaces
                .iter()
                .filter(|row| row["workspaceId"] == id)
                .count(),
            1,
            "Accepted task must have exactly one sidebar row"
        );
    }
    pub(crate) fn update_cloud_account_catalog(&mut self, changed: bool, cx: &mut Context<Self>) {
        if let Some(child) = &self.cloud.child {
            let account = self.cloud_account.clone();
            child.update(cx, |view, cx| {
                view.cloud_account = account;
                cx.notify();
            });
        }
        // Populate cloud-only project headers on the first account read,
        // independently of local chats, installed agents or an open picker.
        if !self.cloud.guest
            && !self.cached_only
            && self.cloud_account["user"].is_object()
            && (changed || self.cloud.account.is_none())
        {
            self.refresh_cloud();
        }
    }
    pub(crate) fn cloud_draft_project(&self) -> Option<Value> {
        match self.cloud.project_id.as_deref() {
            Some(id) => self
                .cloud
                .projects
                .iter()
                .find(|row| row["projectId"] == id)
                .cloned(),
            None => self.cloud.projects.first().cloned(),
        }
    }

    pub(crate) fn start_thread_conversation(&mut self, cx: &mut Context<Self>) {
        if (!self.cloud_server_commands() && !self.require_connection(cx))
            || self.file_navigation_blocked(cx)
            || self.queue_edit_navigation_blocked(cx)
        {
            return;
        }
        let Some(chat) = self.selected.clone() else {
            return;
        };
        if chat["archived"] == true {
            return;
        }
        let root = chat["threadId"]
            .as_str()
            .or_else(|| chat["id"].as_str())
            .unwrap_or_default();
        self.call(methods::CREATE_THREAD_SESSION, json!({"chatId":uuid::Uuid::new_v4().to_string(),"threadId":root,"targetDeviceId":chat["deviceId"],"templateChat":chat}));
        cx.notify();
    }
    pub(crate) fn cloud_project_draft_key(&self) -> Option<String> {
        (self.selected.is_none() && self.cloud.mode).then(|| {
            format!(
                "new-cloud:{}",
                json!([
                    self.cloud.namespace,
                    self.cloud.account,
                    self.cloud.organization,
                    self.cloud.project_id
                ])
            )
        })
    }

    pub fn start_cloud_project_session(
        &mut self,
        project: Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.cloud.guest {
            let _=self.requests.unbounded_send(json!({"ui":"newCloudProjectConversation","projectId":project["projectId"],"accountId":self.cloud.account,"organizationId":self.cloud.organization}));
            return true;
        }
        if self.cloud.create.is_some()
            || (self.cloud.launch.is_some() && self.cloud.accepted.is_none())
        {
            self.error=Some("Resolve the pending task request before starting another. Your draft is preserved.".into());
            cx.notify();
            return false;
        }
        let Some(id) = project["projectId"]
            .as_str()
            .filter(|id| !id.is_empty())
            .map(str::to_owned)
        else {
            return false;
        };
        // Save the old destination's draft before switching to the cloud.
        if !self.new_chat_for(true, cx) {
            return false;
        }
        self.close_cloud();
        self.cloud.mode = true;
        self.cloud.guest = false;
        self.cloud.cached = false;
        self.cloud.guest_workspace = None;
        self.cloud.guest_status = Value::Null;
        self.cached_only = false;
        self.settings = false;
        self.project = None;
        self.cloud.project_id = Some(id.clone());
        self.cloud.project = self
            .cloud
            .projects
            .iter()
            .position(|row| row["projectId"] == id)
            .unwrap_or(usize::MAX);
        self.model_picker = false;
        self.access_picker = false;
        self.add_menu = false;
        self.cloud_branch
            .update(cx, |value, cx| value.set_value(String::new(), cx));
        self.error = None;
        self.refresh_cloud();
        self.watch_cloud_account_events();
        self.restore_draft(cx);
        window.focus(&self.input.focus_handle(cx), cx);
        cx.notify();
        true
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::prelude::v1::test;
    #[gpui::test]
    fn account_reads_route_to_the_controller_with_immutable_scope(cx: &mut TestAppContext) {
        cx.update(initialize);
        let (requests, mut sent) = futures::channel::mpsc::unbounded();
        let (_events, frames) = futures::channel::mpsc::unbounded();
        let window = cx.add_window(|window, cx| {
            WorkspaceView::new(
                Transport {
                    requests,
                    events: frames,
                },
                window,
                cx,
            )
        });
        window.update(cx, |view, _, _| {
            view.cloud.account = Some("owner".into());
            view.cloud.organization = Some("team".into());
            for (i, method) in [methods::CLOUD_ACCOUNT_STATUS, methods::CLOUD_CONTEXT, methods::CLOUD_WORKSPACES, methods::CLOUD_EVENTS].into_iter().enumerate() {
                assert!(view.forward_cloud_account_control(&json!({"id":i+100,"method":method,"params":{"expectedAccountId":"spoofed","expectedOrganizationId":"other"}})));
                let frame = sent.try_recv().unwrap();
                assert_eq!(frame["method"], method);
                assert_eq!(frame["params"]["expectedAccountId"], "owner");
                assert_eq!(frame["params"]["expectedOrganizationId"], "team");
                assert_eq!(view.cloud.forwards.get(&frame["id"].as_u64().unwrap()).unwrap().0, (i+100) as u64);
                assert_ne!(frame["method"], methods::CLOUD_RPC, "Account reads must never reach guest credentials");
            }
            assert!(!view.forward_cloud_account_control(&json!({"id":200,"method":methods::MUTATE,"params":{}})));
        }).unwrap();
    }

    #[gpui::test]
    fn live_child_keeps_parent_authority_and_initial_session_on_account_status(
        cx: &mut TestAppContext,
    ) {
        cx.update(initialize);
        let (requests, _sent) = futures::channel::mpsc::unbounded();
        let (_events, frames) = futures::channel::mpsc::unbounded();
        let window = cx.add_window(|window, cx| {
            WorkspaceView::new(
                Transport {
                    requests,
                    events: frames,
                },
                window,
                cx,
            )
        });
        window.update(cx, |view, window, cx| {
            view.cloud_account = json!({"user":{"id":"owner"},"orgId":null});
            view.cloud.account = Some("owner".into());
            view.cloud.namespace = Some("https://cloud.example/".into());
            view.cloud.authority = true;
            view.cloud.connections = vec![json!({"connectionId":"selected","agent":"codex","status":"connected"})];
            view.cloud.connection = Some("selected".into());
            view.cloud.capabilities = json!({"canAllocate":true});
            view.cloud.provider_catalogs = json!({"boat":{"id":"codex"}});
            view.cloud.target = Some(json!({"workspaceId":"workspace","chatId":"root","initialSessionId":"initial","title":"Accepted task","state":"setup"}));
            view.cloud.status = json!({"state":"ready","epoch":1});
            let (requests, _sent) = futures::channel::mpsc::unbounded();
            let (_events, events) = futures::channel::mpsc::unbounded();
            let child = cx.new(|cx| WorkspaceView::new(Transport { requests, events }, window, cx));
            child.update(cx, |child, _| view.initialize_live_cloud_child(child));
            child.update(cx, |child, cx| {
                assert_eq!(child.cloud.namespace.as_deref(),Some("https://cloud.example/"));
                assert_eq!(child.cloud_account["user"]["id"],"owner");
                assert_eq!(child.cloud.connection.as_deref(),Some("selected"));
                assert_eq!(child.cloud.connections.len(),1);
                assert_eq!(child.cloud.capabilities["canAllocate"],true);
                assert_eq!(child.cloud.initial_session.as_deref(),Some("initial"));
                assert_eq!(child.cloud.metadata_thread.as_deref(),Some("root"));
                child.pending.insert(70,methods::CLOUD_ACCOUNT_STATUS.into());
                child.receive(json!({"id":70,"item":{"user":{"id":"owner"},"orgId":null}}),cx);
                assert!(child.cloud.guest);
                assert_eq!(child.cloud.initial_session.as_deref(),Some("initial"));
                child.connected=true;
                child.pending.insert(71,methods::WATCH_CHATS.into());
                child.receive(json!({"id":71,"item":[{"id":"root","title":"Task"}]}),cx);
                assert!(child.chats.is_empty(), "The metadata thread is never selectable while the initial session arrives");
                assert!(child.selected.is_none());
                assert_eq!(child.cloud.initial_session.as_deref(),Some("initial"));
                child.receive(json!({"id":71,"item":[{"id":"root","title":"Task"},{"id":"initial","threadId":"root","title":null,"config":{"harness":"codex","model":"selected"}}]}),cx);
                assert_eq!(child.selected.as_ref().unwrap()["id"],"initial");
                assert_eq!(child.selected.as_ref().unwrap()["title"],"Accepted task");
                assert_eq!(child.chats.len(),1);
                assert!(child.cloud.initial_session.is_none());
                child.receive(json!({"id":71,"item":[{"id":"root","title":"Task"},{"id":"initial","threadId":"root","title":"Agent title","config":{"harness":"codex","model":"selected"}},{"id":"followup","threadId":"root","title":null}]}),cx);
                assert_eq!(child.selected.as_ref().unwrap()["id"],"initial");
                assert_eq!(child.selected.as_ref().unwrap()["title"],"Agent title");
                assert_eq!(child.chats.len(),2);
                assert!(child.chats.iter().find(|chat| chat["id"]=="followup").unwrap()["title"].is_null());
                assert!(child.cloud.guest);
            });
            drop(child);
        }).unwrap();
        cx.run_until_parked();
    }

    #[gpui::test]
    fn legacy_cloud_root_conversations_and_local_chats_remain_visible(cx: &mut TestAppContext) {
        cx.update(initialize);
        let (requests, _sent) = futures::channel::mpsc::unbounded();
        let (_events, frames) = futures::channel::mpsc::unbounded();
        let window = cx.add_window(|window, cx| {
            WorkspaceView::new(
                Transport {
                    requests,
                    events: frames,
                },
                window,
                cx,
            )
        });
        window.update(cx, |view, window, cx| {
            view.cloud.target=Some(json!({"workspaceId":"workspace","chatId":"root","initialSessionId":"root"}));
            let (requests, _sent)=futures::channel::mpsc::unbounded();
            let (_events, events)=futures::channel::mpsc::unbounded();
            let child=cx.new(|cx| WorkspaceView::new(Transport {requests,events},window,cx));
            child.update(cx, |child, _| view.initialize_live_cloud_child(child));
            let rows=json!([{"id":"root","title":"Existing conversation"},{"id":"followup","threadId":"root"}]);
            assert_eq!(child.read(cx).receive_cloud_conversations(&rows),rows.as_array().unwrap().clone());
            view.cloud.metadata_thread=Some("root".into());
            assert_eq!(view.receive_cloud_conversations(&rows),rows.as_array().unwrap().clone(),"Local navigation never hides a real root conversation");
        }).unwrap();
    }

    #[gpui::test]
    fn unexpected_guest_signed_out_status_blocks_commands_without_becoming_local(
        cx: &mut TestAppContext,
    ) {
        cx.update(initialize);
        let (requests, _sent) = futures::channel::mpsc::unbounded();
        let (_events, frames) = futures::channel::mpsc::unbounded();
        let window = cx.add_window(|window, cx| {
            WorkspaceView::new(
                Transport {
                    requests,
                    events: frames,
                },
                window,
                cx,
            )
        });
        window
            .update(cx, |view, _, cx| {
                view.cloud.guest = true;
                view.cloud.account = Some("owner".into());
                view.cloud.initial_session = Some("initial".into());
                view.cloud.guest_status =
                    json!({"commandApiAuthorized":true,"taskCommandCanEdit":true});
                view.pending
                    .insert(70, methods::CLOUD_ACCOUNT_STATUS.into());
                view.receive(json!({"id":70,"item":{"user":null,"orgId":null}}), cx);
                assert!(
                    view.cloud.guest,
                    "Unauthenticated guest status cannot reclassify the document as local"
                );
                assert_eq!(view.cloud.initial_session.as_deref(), Some("initial"));
                assert_eq!(view.cloud.guest_status["commandApiAuthorized"], false);
                assert_eq!(view.cloud.guest_status["taskCommandCanEdit"], false);
                assert!(view.error.as_deref().unwrap().contains("account changed"));
            })
            .unwrap();
    }

    #[gpui::test]
    fn cloud_only_project_starts_a_draft_without_local_chats_or_device(cx: &mut TestAppContext) {
        cx.update(initialize);
        let (requests, mut sent) = futures::channel::mpsc::unbounded();
        let (_events, frames) = futures::channel::mpsc::unbounded();
        let window = cx.add_window(|window, cx| {
            WorkspaceView::new(
                Transport {
                    requests,
                    events: frames,
                },
                window,
                cx,
            )
        });
        window.update(cx,|view,window,cx|{
            view.connected=false;view.workspace_synced=false;view.device_id=None;view.chats.clear();view.spaces.clear();view.providers.clear();
            view.cloud.account=Some("owner".into());view.cloud.namespace=Some("https://cloud.example/".into());
            let project=json!({"projectId":"cloud-only","accountId":"owner","displayName":"Cloud repository","repositoryIdentity":"github.com/example/repository","defaultBranch":"main","providerId":"boxd"});
            assert!(view.start_cloud_project_session(project,window,cx),"Cloud draft must not require a local engine/device/chat");
            assert!(!view.settings);assert!(view.cloud.mode);assert!(view.selected.is_none());assert!(view.chats.is_empty());assert!(view.cloud.create.is_none());
        }).unwrap();
        while let Ok(frame) = sent.try_recv() {
            assert_ne!(frame["method"], methods::CLOUD_CREATE);
            assert_ne!(frame["method"], methods::MUTATE);
        }
    }
}
