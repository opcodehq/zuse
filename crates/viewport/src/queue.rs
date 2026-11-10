//! Host-scoped queue actions wait for confirmation and a fresh queue read.
use super::*;
use zuse_protocol::presentation::{QueueAction, chat_request, queue_row_details};

pub(super) struct PendingQueueAction {
    pub chat: String,
    pub action: QueueAction,
}
impl WorkspaceView {
    pub(super) fn pause_cloud_queue(&mut self, paused: bool, cx: &mut Context<Self>) {
        if !self.cloud_server_commands() || self.queue_action_blocked() {
            return;
        }
        let Some(chat) = self
            .selected
            .as_ref()
            .and_then(|v| v["id"].as_str())
            .map(str::to_owned)
        else {
            return;
        };
        let request = json!({"commandId":uuid::Uuid::new_v4().to_string(),"sessionId":chat,"action":{"type":"pause-queue","paused":paused}});
        let id = self.call(methods::CLOUD_TASK_COMMAND, json!({"request":request}));
        self.queue_pause_request = Some((id, chat));
        cx.notify();
    }
    pub(super) fn receive_queue_pause(
        &mut self,
        id: u64,
        frame: &Value,
        cx: &mut Context<Self>,
    ) -> bool {
        if self
            .queue_pause_request
            .as_ref()
            .is_none_or(|(pending, _)| *pending != id)
        {
            return false;
        }
        if frame.get("ok").is_none() && frame.get("err").is_none() && frame["done"] != true {
            return true;
        }
        let (_, chat) = self.queue_pause_request.take().unwrap();
        self.pending.remove(&id);
        if self.selected.as_ref().and_then(|v| v["id"].as_str()) != Some(chat.as_str()) {
            return true;
        }
        if frame.get("err").is_some() {
            self.error = Some(
                "The queue change was not confirmed. Refresh its current state before retrying."
                    .into(),
            );
        }
        if let Some(selected) = self.selected.clone() {
            self.queue_unconfirmed.insert(chat);
            self.refresh_queue_snapshot(&selected);
        }
        cx.notify();
        true
    }
    pub(super) fn queued_primary_action(
        &self,
        item: &Value,
    ) -> Option<zuse_protocol::presentation::QueuePrimaryAction> {
        let details = queue_row_details(item);
        let provider = self.composer_provider()?;
        let supports = provider["supportsSteering"].as_bool()?;
        let mode = provider["steeringMode"].as_str()?;
        if !matches!(mode, "step-boundary" | "turn-boundary") {
            return None;
        }
        let mid_turn = supports && mode == "step-boundary";
        zuse_protocol::presentation::queue_primary_action(
            self.queue_action_blocked() || !details.valid_id || details.blocked.is_some(),
            true,
            details.attachment_count > 0,
            mid_turn,
        )
    }

    pub(super) fn submit_composer(
        &mut self,
        secondary: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.browser_voice_replaces_composer() {
            return;
        }
        if secondary
            && self.queue_edit.is_none()
            && !self.settings
            && self.input.read(cx).focus_handle(cx).is_focused(window)
            && self.input.read(cx).value(cx).trim().is_empty()
            && self.attachments.is_empty()
        {
            if self.uploading == 0 && self.creating.is_none() {
                if let Some(item) = self.queue.last().cloned() {
                    if let Some(action) = self.queued_primary_action(&item) {
                        self.queue_action(action.action(), &item, None, cx);
                    }
                }
            }
            return;
        }
        self.submit(&Submit, window, cx);
    }

    pub(super) fn move_queued_drop(
        &mut self,
        payload: &crate::queue_render::QueueDrag,
        target: &str,
        cx: &mut Context<Self>,
    ) {
        if self.selected.as_ref().is_none_or(|chat| {
            chat["id"].as_str() != Some(payload.chat.as_str())
                || chat["deviceId"].as_str() != payload.host.as_deref()
        }) || payload.id == target
        {
            return;
        }
        let Some(position) = self.queue.iter().position(|item| item["id"] == target) else {
            return;
        };
        let Some(item) = self
            .queue
            .iter()
            .find(|item| item["id"] == payload.id)
            .cloned()
        else {
            return;
        };
        self.queue_action(QueueAction::Move, &item, Some(position), cx);
    }

    pub(super) fn queue_action_blocked(&self) -> bool {
        self.queue_pause_request.is_some()
            || self.queue_edit.is_some()
            || self.model_change_pending_for_selection()
            || self.access_change_pending_for_selection()
            || self.transcript_blocked()
            || (!self.connected && !self.cloud_server_commands())
            || (!self.workspace_synced && !self.cloud_server_commands())
            || self
                .selected
                .as_ref()
                .and_then(|chat| chat["id"].as_str())
                .is_none_or(|chat| {
                    self.queue_unconfirmed.contains(chat)
                        || self
                            .queue_actions
                            .values()
                            .any(|action| action.chat == chat)
                })
    }
    pub(super) fn queue_action(
        &mut self,
        action: QueueAction,
        item: &Value,
        position: Option<usize>,
        cx: &mut Context<Self>,
    ) {
        if (!self.cloud_server_commands() && !self.require_connection(cx))
            || self.queue_action_blocked()
        {
            return;
        }
        let Some(chat) = self.selected.clone() else {
            return;
        };
        let Some(chat_id) = chat["id"].as_str().map(str::to_owned) else {
            return;
        };
        let Some(current) = self.queue.iter().find(|row| row["id"] == item["id"]) else {
            return;
        };
        if !queue_row_details(current).allows(action) {
            self.error = Some("This queued message is blocked for editing/review, or needs an explicit send with its attachments.".into());
            cx.notify();
            return;
        }
        let mut params = json!({"id":current["id"],"revision":current["revision"]});
        if action == QueueAction::Move {
            let Some(position) = position.filter(|index| *index < self.queue.len()) else {
                return;
            };
            params["toIndex"] = json!(position);
        }
        let request = self.call(action.method(), chat_request(&chat, params));
        self.queue_actions.insert(
            request,
            PendingQueueAction {
                chat: chat_id,
                action,
            },
        );
        cx.notify();
    }
    pub(super) fn receive_queue_action(
        &mut self,
        id: u64,
        frame: &Value,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.queue_actions.contains_key(&id) {
            return false;
        }
        if frame.get("ok").is_none() && frame.get("err").is_none() && frame["done"] != true {
            return true;
        }
        let pending = self.queue_actions.remove(&id).unwrap();
        self.pending.remove(&id);
        let error = if let Some(error) = frame["err"].as_str() {
            Some(error.to_owned())
        } else {
            match frame.get("ok").and_then(|reply| pending.action.confirmed(reply)) {
                Some(true) => None,
                Some(false) => Some("That queued message changed or was already taken. Refreshing its current state.".into()),
                None => Some("The host did not confirm the queue action. Check the refreshed queue before trying again.".into()),
            }
        };
        if let Some(error) = error {
            self.error = Some(error);
        }
        if let Some(chat) = self
            .selected
            .clone()
            .filter(|chat| chat["id"] == pending.chat)
        {
            self.refresh_queue_snapshot(&chat);
        } else {
            self.queue_unconfirmed.insert(pending.chat);
        }
        cx.notify();
        true
    }
    pub(super) fn refresh_queue_snapshot(&mut self, chat: &Value) {
        if let Some(id) = chat["id"].as_str() {
            self.queue_unconfirmed.insert(id.to_owned());
        }
        if !self.connected && !self.cloud_server_commands() {
            return;
        }
        // An editor can retain the previous host after a handoff. Read from the
        // current chat authority; never send its old lease to the new host.
        let Some(chat) = self
            .selected
            .clone()
            .filter(|selected| selected["id"] == chat["id"])
        else {
            return;
        };
        if let Some(watch) = self.queue_watch.take() {
            self.cancel(watch);
        }
        self.queue_watch = Some(self.call(methods::WATCH_QUEUE, chat_request(&chat, json!({}))));
    }
    pub(super) fn rebind_chat_host(&mut self, cx: &mut Context<Self>) {
        let Some(chat) = self.selected.clone() else {
            return;
        };
        let requests = self
            .queue_actions
            .iter()
            .filter(|(_, action)| chat["id"] == action.chat)
            .map(|(id, _)| *id)
            .collect::<Vec<_>>();
        for id in requests {
            self.queue_actions.remove(&id);
            self.cancel(id);
        }
        self.interrupt_queue_edit(cx);
        self.reset_project_actions(cx);
        self.detach_browser_terminal(cx);
        self.prefetch_model_catalogs();
        self.stop_transcript();
        // Until the new snapshot arrives, old approval cards must not submit
        // answers to a different host. Canceled subscription frames are ignored.
        self.messages.clear();
        self.reset_context_status();
        self.queue.clear();
        let inputs = self.input_pending.keys().copied().collect::<Vec<_>>();
        for id in inputs {
            self.cancel(id);
        }
        self.reset_inputs(cx);
        self.reset_tool_results();
        self.reset_previews(cx);
        self.refresh_queue_snapshot(&chat);
        self.begin_transcript(cx);
        self.connection_notice = Some("This chat moved to another computer. Check any action already sent before trying it again.".into());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::prelude::v1::test;
    struct Fixture {
        window: WindowHandle<WorkspaceView>,
        requests: UnboundedReceiver<Value>,
        _events: UnboundedSender<Value>,
    }
    fn fixture(cx: &mut TestAppContext) -> Fixture {
        cx.update(initialize);
        let (requests, rx) = futures::channel::mpsc::unbounded();
        let (events, frames) = futures::channel::mpsc::unbounded();
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
            view.connected=true;view.workspace_synced=true;view.connection_epoch=1;view.device_id=Some("viewer".into());
            let chat=json!({"id":"chat","deviceId":"host","config":{"harness":"claude-code","sandbox":"workspace-write"}});
            view.selected=Some(chat.clone());view.chats=vec![chat];view.begin_transcript(cx);let watch=view.transcript_watch.unwrap();view.receive(json!({"id":watch,"item":{"reset":[]}}),cx);view.model_confirmed=true;
            view.model_host.target=Some("host".into());view.model_host.confirmed=true;
            view.model_host.rows=vec![json!({"id":"claude-code","installed":true,"supportsSteering":true,"steeringMode":"step-boundary"})];
            view.sessions=vec![json!({"chatId":"chat","status":"working"})];
            view.queue=vec![json!({"id":"older","text":"first"}),json!({"id":"newest","text":"second"})];
            window.focus(&view.input.read(cx).focus_handle(cx),cx);
        }).unwrap();
        Fixture {
            window,
            requests: rx,
            _events: events,
        }
    }
    fn mutations(f: &mut Fixture) -> Vec<Value> {
        let mut out = vec![];
        while let Ok(v) = f.requests.try_recv() {
            if matches!(
                v["method"].as_str(),
                Some(
                    methods::STEER_QUEUED_MESSAGE_NOW
                        | methods::SEND_QUEUED_MESSAGE_NOW
                        | methods::MOVE_QUEUED_MESSAGE
                        | methods::QUEUE_MESSAGE
                        | methods::QUEUE_COMMAND
                )
            ) {
                out.push(v);
            }
        }
        out
    }

    #[gpui::test]
    fn queue_shortcut_uses_the_newest_rows_actual_delivery_action(cx: &mut TestAppContext) {
        for attachments in [false, true] {
            let mut f = fixture(cx);
            f.window
                .update(cx, |view, window, cx| {
                    if attachments {
                        view.queue[1]["attachments"] = json!(["/host/image.png"]);
                    }
                    view.submit_composer(false, window, cx);
                    view.submit_composer(true, window, cx);
                    view.submit_composer(true, window, cx); // acknowledgement is still pending
                })
                .unwrap();
            let writes = mutations(&mut f);
            assert_eq!(writes.len(), 1);
            assert_eq!(
                writes[0]["method"],
                if attachments {
                    methods::SEND_QUEUED_MESSAGE_NOW
                } else {
                    methods::STEER_QUEUED_MESSAGE_NOW
                }
            );
            assert_eq!(
                writes[0]["params"],
                json!({"chatId":"chat","targetDeviceId":"host","id":"newest"})
            );
        }
    }
    #[gpui::test]
    fn queue_shortcut_keeps_content_queued_and_never_skips_a_blocked_latest_row(
        cx: &mut TestAppContext,
    ) {
        let mut f = fixture(cx);
        f.window
            .update(cx, |view, window, cx| {
                view.queue[1]["deliveryGate"] = json!({"kind":"reviewRequired"});
                view.submit_composer(true, window, cx);
                view.queue[1]["deliveryGate"] = Value::Null;
                view.access_unconfirmed = Some("chat".into());
                view.submit_composer(true, window, cx);
                view.access_unconfirmed = None;
                view.input.update(cx, |input, cx| {
                    input.set_value("continue carefully".into(), cx)
                });
                view.attachments = vec![json!({"path":"/host/context.txt"})];
            })
            .unwrap();
        assert!(mutations(&mut f).is_empty());
        f.window
            .update(cx, |view, window, cx| {
                view.submit_composer(true, window, cx)
            })
            .unwrap();
        let writes = mutations(&mut f);
        assert_eq!(writes.len(), 1);
        assert_eq!(writes[0]["method"], methods::QUEUE_MESSAGE);
        assert_eq!(
            writes[0]["params"],
            json!({"chatId":"chat","targetDeviceId":"host","text":"continue carefully","holdForTurnEnd":true,"attachments":["/host/context.txt"]})
        );
    }
    #[gpui::test]
    fn queue_primary_label_follows_execution_host_and_drag_rechecks_row_ids(
        cx: &mut TestAppContext,
    ) {
        let mut f = fixture(cx);
        f.window
            .update(cx, |view, _, cx| {
                use zuse_protocol::presentation::QueuePrimaryAction;
                assert_eq!(
                    view.queued_primary_action(&view.queue[0]),
                    Some(QueuePrimaryAction::Steer)
                );
                view.model_host.rows[0]["steeringMode"] = json!("turn-boundary");
                assert_eq!(
                    view.queued_primary_action(&view.queue[0]),
                    Some(QueuePrimaryAction::SendNext)
                );
                view.model_host.confirmed = false;
                assert_eq!(view.queued_primary_action(&view.queue[0]), None);
                let mut drag = crate::queue_render::QueueDrag {
                    chat: "chat".into(),
                    host: Some("retired-host".into()),
                    id: "older".into(),
                    title: "first".into(),
                };
                view.move_queued_drop(&drag, "newest", cx);
                drag.host = Some("host".into());
                view.move_queued_drop(&drag, "older", cx);
                view.move_queued_drop(&drag, "removed", cx);
                view.queue
                    .insert(0, json!({"id":"inserted","text":"new row"}));
                view.move_queued_drop(&drag, "newest", cx);
            })
            .unwrap();
        let writes = mutations(&mut f);
        assert_eq!(writes.len(), 1);
        assert_eq!(
            writes[0]["params"],
            json!({"chatId":"chat","targetDeviceId":"host","id":"older","toIndex":2})
        );
    }
    #[gpui::test]
    fn cloud_resume_queue_uses_typed_api_and_waits_for_authoritative_snapshot(
        cx: &mut TestAppContext,
    ) {
        let mut f = fixture(cx);
        f.window.update(cx,|view,_,cx|{
            view.cloud.guest=true;view.cloud.guest_status=json!({"commandDeliveryMode":"server-admitted-v2","commandAdmissionVersion":2,"commandApiAuthorized":true,"taskCommandCanEdit":true});
            view.connected=false;view.workspace_synced=false;view.queue.clear();view.queue_unconfirmed.clear();view.queue_paused=true;
            view.pause_cloud_queue(false,cx);let (id,chat)=view.queue_pause_request.clone().expect("paused cloud queue must be resumable without runtime connection");assert_eq!(chat,"chat");
            view.pause_cloud_queue(false,cx);assert_eq!(view.queue_pause_request.as_ref().unwrap().0,id,"only one pending logical resume action");
            view.receive(json!({"id":id,"ok":{"commandId":"server","durablyAdmitted":true,"paused":false}}),cx);
            assert!(view.queue_paused,"wait for actual queue projection, not an optimistic toggle");
            let watch=view.queue_watch.unwrap();view.receive(json!({"id":watch,"item":{"items":[],"paused":false}}),cx);assert!(!view.queue_paused);
        }).unwrap();
        let mut mutations = vec![];
        while let Ok(request) = f.requests.try_recv() {
            if request["method"] == methods::CLOUD_TASK_COMMAND {
                mutations.push(request);
            }
        }
        assert_eq!(mutations.len(), 1);
        let request = &mutations[0]["params"]["request"];
        assert_eq!(request["sessionId"], "chat");
        assert_eq!(
            request["action"],
            json!({"type":"pause-queue","paused":false})
        );
        assert!(uuid::Uuid::parse_str(request["commandId"].as_str().unwrap()).is_ok());
    }

    fn selected_cloud_fixture(cx: &mut TestAppContext, version: u32, cached: bool) -> Fixture {
        let f = fixture(cx);
        f.window.update(cx,|view,_,cx| {
            view.cloud.guest=true;view.cloud.cached=cached;view.connected=!cached;
            view.cloud.guest_status=json!({"commandDeliveryMode":"server-admitted-v2","commandAdmissionVersion":2,"commandApiAuthorized":true,"taskCommandCanEdit":true,"queueRunSelectionVersion":version});
            view.selected.as_mut().unwrap()["config"]["harness"]=json!("grok");
            view.selected.as_mut().unwrap()["config"]["model"]=json!("old-registered-model");
            if cached {
                let mut chat=view.selected.clone().unwrap();chat["createdAt"]=json!("2026-01-01T00:00:00Z");chat["archived"]=json!(false);chat["config"]["reasoning"]=Value::Null;
                let head:zuse_model::cloud::Head=serde_json::from_value(json!({"schemaVersion":1,"payloadFormat":"zuse-cloud-admission-v1","workspaceId":"queue-workspace","sessionId":"chat","runtimeGeneration":1,"cursor":{"epoch":"epoch","version":1},"chat":chat,"conversations":[],"transcript":{"reset":[]},"queue":[],"queuePaused":false})).unwrap();
                view.cloud.guest_workspace=Some(head.workspace_id.clone());view.install_cached_history(&head);view.cached_only=false;view.selected.as_mut().unwrap()["archived"]=json!(false);
            }
            view.model_host.rows=vec![json!({"id":"grok","installed":true,"supportsSteering":true,"steeringMode":"step-boundary"})];
            view.model=Some("chosen-grok".into());view.reasoning=Some(zuse_protocol::ReasoningLevel::High);view.model_options=serde_json::from_value(json!({"speed":"fast"})).unwrap();view.sandbox=zuse_protocol::SandboxLevel::WorkspaceWrite;
            view.input.update(cx,|input,cx|input.set_value("Keep selected config".into(),cx));view.attachments=vec![json!({"path":"/host/context.txt"})];
        }).unwrap();
        f
    }
    #[gpui::test]
    fn cloud_queue_keeps_selected_model_and_refuses_old_server_without_losing_draft(
        cx: &mut TestAppContext,
    ) {
        for version in [0, 1] {
            let mut f = selected_cloud_fixture(cx, version, false);
            f.window
                .update(cx, |view, window, cx| {
                    assert_eq!(
                        view.input.read(cx).value(cx),
                        "Keep selected config",
                        "the real deferred editor write must settle before submission"
                    );
                    view.submit_composer(true, window, cx);
                    if version == 0 {
                        assert_eq!(view.input.read(cx).value(cx), "Keep selected config");
                        assert_eq!(view.attachments.len(), 1);
                        assert!(
                            view.error
                                .as_deref()
                                .unwrap()
                                .contains("draft is preserved")
                        );
                    }
                })
                .unwrap();
            let writes = mutations(&mut f);
            if version == 0 {
                assert!(writes.is_empty());
            } else {
                assert_eq!(writes.len(), 1);
                assert_eq!(writes[0]["method"], methods::QUEUE_MESSAGE);
                assert_eq!(
                    writes[0]["params"]["runSelection"],
                    json!({"harness":"grok","model":"chosen-grok","reasoning":"high","modelOptions":{"speed":"fast"},"sandbox":"workspace-write"})
                );
                assert_eq!(writes[0]["params"]["text"], "Keep selected config");
                assert_eq!(
                    writes[0]["params"]["attachments"],
                    json!(["/host/context.txt"])
                );
            }
        }
    }
    #[gpui::test]
    fn cached_cloud_direct_run_uses_current_selection_without_requiring_queue_capability(
        cx: &mut TestAppContext,
    ) {
        let mut f = selected_cloud_fixture(cx, 0, true);
        f.window
            .update(cx, |view, window, cx| {
                assert_eq!(view.input.read(cx).value(cx), "Keep selected config");
                view.submit_composer(true, window, cx);
            })
            .unwrap();
        let writes = mutations(&mut f);
        assert_eq!(writes.len(), 1);
        assert_eq!(writes[0]["method"], methods::QUEUE_COMMAND);
        assert_eq!(writes[0]["params"]["wake"], "explicit-resume");
        let run = &writes[0]["params"]["command"]["request"];
        assert_eq!(run["harness"], "grok");
        assert_eq!(run["model"], "chosen-grok");
        assert_eq!(run["reasoning"], "high");
        assert_eq!(run["modelOptions"], json!({"speed":"fast"}));
        assert_eq!(run["sandbox"], "workspace-write");
        assert_eq!(run["autoApprove"], false);
        assert_eq!(run["cwd"], ".");
    }
    #[gpui::test]
    fn freshly_hydrated_cached_head_sends_its_authenticated_config_without_local_catalog_or_separate_resume_rpc(
        cx: &mut TestAppContext,
    ) {
        for variant in ["fresh", "pending-edit", "foreign-target"] {
            let mut f = selected_cloud_fixture(cx, 0, true);
            f.window
                .update(cx, |view, _, _| {
                    // Match the real cached_cloud_view hydration: no live host confirmation/catalog.
                    view.model_confirmed = false;
                    view.model_host = Default::default();
                    view.providers.clear();
                    view.connected = false;
                    view.model = Some("unrelated-local-model".into());
                    view.reasoning = Some(zuse_protocol::ReasoningLevel::High);
                    if variant == "pending-edit" {
                        view.model_dirty = true;
                    }
                    if variant == "foreign-target" {
                        view.cloud.guest_workspace = Some("another-workspace".into());
                    }
                })
                .unwrap();
            f.window
                .update(cx, |view, window, cx| {
                    assert_eq!(view.input.read(cx).value(cx), "Keep selected config");
                    view.submit_composer(true, window, cx);
                    if variant != "fresh" {
                        assert_eq!(view.input.read(cx).value(cx), "Keep selected config");
                        assert_eq!(view.attachments.len(), 1);
                    }
                })
                .unwrap();
            let mut writes = Vec::new();
            while let Ok(frame) = f.requests.try_recv() {
                assert_ne!(
                    frame["ui"], "resumeCloud",
                    "cached API admission must not request a separate wake"
                );
                if matches!(
                    frame["method"].as_str(),
                    Some(methods::QUEUE_COMMAND | methods::QUEUE_MESSAGE)
                ) {
                    writes.push(frame);
                }
            }
            if variant != "fresh" {
                assert!(writes.is_empty());
            } else {
                assert_eq!(writes.len(), 1);
                assert_eq!(writes[0]["method"], methods::QUEUE_COMMAND);
                assert_eq!(writes[0]["params"]["wake"], "explicit-resume");
                let run = &writes[0]["params"]["command"]["request"];
                assert_eq!(run["harness"], "grok");
                assert_eq!(run["model"], "old-registered-model");
                assert_eq!(run["reasoning"], Value::Null);
                assert_eq!(run["modelOptions"], json!({}));
                assert_eq!(run["sandbox"], "workspace-write");
            }
        }
    }
}
