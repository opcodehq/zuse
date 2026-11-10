//! Rebuild cloud reads after the controller link changes, preserving the task view.
use super::*;

impl WorkspaceView {
    /// A confirmed foreground admission owns compute wake. This only attaches
    /// the current view to that operation; it never sends a lifecycle command.
    pub(super) fn attach_admitted_cloud_run(&mut self, reply: &Value, cx: &mut Context<Self>) {
        let reply = reply.get("data").unwrap_or(reply);
        let hint = &reply["passiveAttachment"];
        if reply["durablyAdmitted"] != true
            || hint["mode"] != "passive"
            || hint["commandId"] != reply["commandId"]
            || hint["commandId"].as_str().is_none_or(str::is_empty)
            || hint["accountId"].as_str() != self.cloud.account.as_deref()
            || self.cloud.account.is_none()
            || hint["organizationId"].as_str() != self.cloud.organization.as_deref()
            || self
                .cloud
                .target
                .as_ref()
                .is_none_or(|target| target["workspaceId"] != hint["workspaceId"])
            || self.cloud.child.as_ref().is_none_or(|child| {
                child
                    .read(cx)
                    .selected
                    .as_ref()
                    .is_none_or(|chat| chat["id"] != hint["sessionId"])
            })
        {
            return;
        }
        if self.cloud.status["state"] == "ready" && self.cloud.events.is_some() {
            return;
        }
        // A retired status subscription can still be retained by the old UI.
        // Replace its owner, retaining the existing child and request pump.
        if let Some(id) = self.cloud.status_request.take() {
            self.cancel(id);
        }
        self.connect_cloud_with_intent(false);
    }

    pub(crate) fn interrupt_cloud_transport(&mut self, cx: &mut Context<Self>) {
        if self.cloud.guest || self.cached_only {
            return;
        }
        self.cloud.context = None;
        self.cloud.list = None;
        self.cloud.metadata = None;
        self.cloud.cache_request = None;
        self.cloud.history_request = None;
        self.cloud.sync.metadata = None;
        self.cloud.sync.log_request = None;
        if self.cloud.sync.request.is_some_and(|id| {
            self.pending
                .get(&id)
                .is_some_and(|method| method == methods::CLOUD_SYNC_STATUS)
        }) {
            self.cloud.sync.request = None;
        }
        self.cloud.account_events.request = None;
        self.cloud.account_events.live = false;
        self.cloud.reconnect_transport |= self.cloud.target.is_some()
            && (self.cloud.status_request.is_some() || self.cloud.events.is_some());
        self.cloud.status_request = None;
        let mut interrupted: Vec<_> = self.cloud.forwards.values()
            .map(|(id, method)| json!({"id":id,"mutation":!methods::retryable_read(method,&Value::Null)}))
            .collect();
        interrupted.extend(
            self.cloud
                .read_forwards
                .values()
                .map(|id| json!({"id":id,"mutation":false})),
        );
        self.cloud.read_forwards.clear();
        self.cloud.forwards.clear();
        if let Some(events) = self
            .cloud
            .events
            .as_ref()
            .or(self.cloud.cached_events.as_ref())
        {
            let _ =
                events.unbounded_send(json!({"connection":"retrying","interrupted":interrupted}));
        }
        if !self.cloud.reconnect_transport {
            return;
        }
        self.cloud.connected_epoch = None;
        self.cloud.status =
            json!({"state":"reconnecting","message":"Reconnecting to the computer…"});
        let status = self.cloud_guest_status();
        if let Some(child) = &self.cloud.child {
            child.update(cx, |view, cx| {
                view.cloud.guest_status = status;
                cx.notify();
            });
        }
    }

    pub(crate) fn restore_cloud_transport(&mut self) {
        if self.cloud.guest || self.cached_only || !self.connected || self.cloud.account.is_none() {
            return;
        }
        self.refresh_cloud();
        self.watch_cloud_account_events();
        if self.cloud.reconnect_transport {
            self.cloud.reconnect_transport = false;
            // Observe the original computer/operation. Never replay Resume,
            // a first message, or another potentially executed command.
            self.connect_cloud_with_intent(false);
        }
        if self.cloud.history_request.is_none() {
            if let Some(target) = &self.cloud.target {
                let session = self
                    .cloud
                    .head
                    .as_ref()
                    .map(|head| head.session_id.as_str())
                    .or_else(|| target["initialSessionId"].as_str());
                let body = json!({"workspaceId":target["workspaceId"],"sessionId":session,
                    "incremental":true,"expectedAccountId":self.cloud.account,
                    "expectedOrganizationId":self.cloud.organization});
                self.cloud.history_request = Some(self.call(methods::CLOUD_HISTORY, body));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::prelude::v1::test;

    fn history() -> Value {
        json!({"accountId":"owner","workspace":{"workspaceId":"workspace","state":"ready"},
            "head":{"schemaVersion":1,"payloadFormat":"zuse-cloud-admission-v1",
            "workspaceId":"workspace","sessionId":"initial","runtimeGeneration":1,
            "cursor":{"epoch":"epoch","version":1},
            "chat":{"id":"initial","deviceId":"runtime","archived":false,
                "createdAt":"2026-01-01T00:00:00Z","threadId":"thread"},
            "conversations":[],"transcript":{"reset":[]},"queue":[],"queuePaused":false}})
    }

    #[gpui::test]
    fn controller_reconnect_keeps_live_history_and_rebinds_tools_without_resuming(
        cx: &mut TestAppContext,
    ) {
        cx.update(initialize);
        let (requests, mut outgoing) = futures::channel::mpsc::unbounded();
        let (_events, events) = futures::channel::mpsc::unbounded();
        let window = cx.add_window(|window, cx| {
            WorkspaceView::new(Transport { requests, events }, window, cx)
        });
        window
            .update(cx, |view, window, cx| {
                view.connected = true;
                view.device_id = Some("controller".into());
                view.cloud.account = Some("owner".into());
                view.cloud.namespace = Some("https://cloud.example/".into());
                view.cloud.target = Some(json!({"workspaceId":"workspace","chatId":"thread",
                "initialSessionId":"initial","state":"ready"}));
                view.cloud.head = Some(serde_json::from_value(history()["head"].clone()).unwrap());
                view.cloud.status = json!({"state":"ready","epoch":1});
                view.prepare_cloud_workspace(window, cx);
                let child = view.cloud.child.clone().unwrap();
                child.update(cx, |child, cx| {
                    child.connected = true;
                    child.device_id = Some("runtime".into());
                    child.selected = Some(json!({"id":"initial","deviceId":"runtime","cwd":"."}));
                    child.messages =
                        serde_json::from_value(json!([{"id":"answer","role":"assistant",
                    "parts":[{"id":"text","kind":"text","text":"Retained answer"}],
                    "createdAt":1,"deviceId":"runtime"}]))
                        .unwrap();
                    child
                        .input
                        .update(cx, |input, cx| input.set_value("Unsent draft".into(), cx));
                    child.open_file =
                        Some(json!({"path":"notes.txt","text":"Unsaved editor text"}));
                    child.transcript_scroll.set_offset(point(px(0.), px(-73.)));
                });
                // These are the lost controller-owned reads, not new commands.
                view.cloud.context = Some(view.call(methods::CLOUD_CONTEXT, json!({})));
                view.cloud.history_request = Some(view.call(methods::CLOUD_HISTORY, json!({})));
                view.cloud.status_request = Some(view.call(
                    methods::CLOUD_ENVIRONMENT,
                    json!({"workspaceId":"workspace","interactiveResume":true}),
                ));
                view.cloud.read_forwards.insert(900, 901);
                view.cloud
                    .forwards
                    .insert(902, (903, methods::QUEUE_MESSAGE.into()));
            })
            .unwrap();
        cx.run_until_parked();
        window
            .update(cx, |view, window, cx| {
                let child = view.cloud.child.clone().unwrap();
                while outgoing.try_recv().is_ok() {}
                let child_id = child.entity_id();
                let scroll = child.read(cx).transcript_scroll.offset();
                view.receive_connection(&json!({"connection":"retrying","interrupted":[]}), cx);
                assert!(view.cloud.status_request.is_none());
                assert!(view.cloud.context.is_none());
                assert!(view.cloud.history_request.is_none());
                assert!(view.cloud.forwards.is_empty());
                assert!(view.cloud.read_forwards.is_empty());
                assert_eq!(view.cloud.child.as_ref().unwrap().entity_id(), child_id);
                assert_eq!(view.cloud.status["state"], "reconnecting");
                assert_ne!(child.read(cx).cloud.guest_status["state"], "ready");
                view.receive_connection(&json!({"connection":"connected","epoch":2}), cx);
                assert!(
                    !view.connected,
                    "Controller identity must be verified before reattachment"
                );
                assert!(view.cloud.status_request.is_none());
                view.verify_engine(&json!({"deviceId":"controller"}), cx);
                let environment = view.cloud.status_request.expect("One passive reattachment");
                let mut attachments = vec![];
                while let Ok(frame) = outgoing.try_recv() {
                    if frame["method"] == methods::CLOUD_ENVIRONMENT {
                        attachments.push(frame.clone());
                    }
                    assert_ne!(frame["method"], methods::CLOUD_CREATE);
                    assert_ne!(frame["method"], methods::CLOUD_PAUSE);
                    assert_ne!(frame["method"], methods::CLOUD_TASK_COMMAND);
                    assert_ne!(frame["method"], methods::QUEUE_MESSAGE);
                }
                assert_eq!(attachments.len(), 1);
                assert_eq!(attachments[0]["params"]["interactiveResume"], false);
                // A fresh history response can beat the environment's Ready frame.
                let history_request = view.cloud.history_request.unwrap();
                assert!(view.receive_cloud(&json!({"id":history_request,"ok":history()}), cx));
                view.prepare_cloud_workspace(window, cx);
                assert_eq!(view.cloud.child.as_ref().unwrap().entity_id(), child_id);
                assert!(
                    !child.read(cx).cloud.cached,
                    "History before Ready must not downgrade a live bridge"
                );
                assert_eq!(child.read(cx).input.read(cx).value(cx), "Unsent draft");
                assert_eq!(
                    child.read(cx).open_file.as_ref().unwrap()["text"],
                    "Unsaved editor text"
                );
                assert_eq!(child.read(cx).messages[0].id, "answer");
                assert_eq!(child.read(cx).transcript_scroll.offset(), scroll);
                assert!(view.receive_cloud(
                    &json!({"id":environment,"item":{"state":"ready","epoch":3}}),
                    cx
                ));
                view.prepare_cloud_workspace(window, cx);
                // History after Ready must preserve the same bridge too.
                view.cloud.history_request = Some(910);
                assert!(view.receive_cloud(&json!({"id":910,"ok":history()}), cx));
                view.cached_cloud_view(window, cx);
                assert_eq!(view.cloud.child.as_ref().unwrap().entity_id(), child_id);
                child.update(cx, |child, cx| {
                    assert!(!child.cloud.cached);
                    child.verify_engine(&json!({"deviceId":"runtime"}), cx);
                    assert!(
                        child.require_connection(cx),
                        "Tools route through the restored live bridge"
                    );
                    assert_eq!(child.messages[0].id, "answer");
                    assert_eq!(child.input.read(cx).value(cx), "Unsent draft");
                    assert_eq!(child.transcript_scroll.offset(), scroll);
                });
                assert!(view.cloud.status_request == Some(environment));
            })
            .unwrap();
        window
            .update(cx, |view, window, _| {
                view.close_cloud();
                window.remove_window();
            })
            .unwrap();
        cx.run_until_parked();
    }
    #[gpui::test]
    fn controller_reconnect_refreshes_paused_history_without_waking(cx: &mut TestAppContext) {
        cx.update(initialize);
        let (requests, mut outgoing) = futures::channel::mpsc::unbounded();
        let (_events, events) = futures::channel::mpsc::unbounded();
        let window = cx.add_window(|window, cx| {
            WorkspaceView::new(Transport { requests, events }, window, cx)
        });
        window
            .update(cx, |view, window, cx| {
                view.connected = true;
                view.device_id = Some("controller".into());
                view.cloud.account = Some("owner".into());
                view.cloud.target = Some(json!({"workspaceId":"workspace","chatId":"thread",
                "initialSessionId":"initial","state":"paused","title":"Accepted task"}));
                let mut head = history()["head"].clone();
                let mut root = head["chat"].clone();
                root["id"] = json!("thread");
                root["title"] = json!("Metadata container");
                head["conversations"] = json!([root, head["chat"]]);
                view.cloud.head = Some(serde_json::from_value(head.clone()).unwrap());
                let certified_head =
                    serde_json::to_value(view.cloud.head.as_ref().unwrap()).unwrap();
                view.cloud.status = json!({"state":"paused"});
                view.cached_cloud_view(window, cx);
                let child = view.cloud.child.clone().unwrap();
                let child_id = child.entity_id();
                assert_eq!(
                    child.read(cx).chats.len(),
                    1,
                    "Fresh paused history hides only the exact metadata root"
                );
                assert_eq!(child.read(cx).chats[0]["id"], "initial");
                assert_eq!(
                    child.read(cx).selected.as_ref().unwrap()["title"],
                    "Accepted task"
                );
                assert_eq!(
                    serde_json::to_value(view.cloud.head.as_ref().unwrap()).unwrap(),
                    certified_head,
                    "Presentation never rewrites the signed Head"
                );
                while outgoing.try_recv().is_ok() {}
                view.receive_connection(&json!({"connection":"retrying","interrupted":[]}), cx);
                view.receive_connection(&json!({"connection":"connected","epoch":2}), cx);
                view.verify_engine(&json!({"deviceId":"controller"}), cx);
                let id = view
                    .cloud
                    .history_request
                    .expect("Refresh saved history after identity verification");
                let mut saved = history();
                saved["workspace"]["state"] = json!("paused");
                assert!(view.receive_cloud(&json!({"id":id,"ok":saved}), cx));
                view.prepare_cloud_workspace(window, cx);
                assert_eq!(view.cloud.child.as_ref().unwrap().entity_id(), child_id);
                assert!(child.read(cx).cloud.cached);
                assert!(view.cloud.status_request.is_none());
                while let Ok(frame) = outgoing.try_recv() {
                    assert_ne!(
                        frame["method"],
                        methods::CLOUD_ENVIRONMENT,
                        "Reading paused history must never acquire or resume compute"
                    );
                    assert_ne!(frame["method"], methods::CLOUD_CREATE);
                    assert_ne!(frame["method"], methods::CLOUD_TASK_COMMAND);
                    assert_ne!(frame["method"], methods::QUEUE_MESSAGE);
                }
            })
            .unwrap();
        window
            .update(cx, |view, window, _| {
                view.close_cloud();
                window.remove_window();
            })
            .unwrap();
        cx.run_until_parked();
    }
    #[gpui::test]
    fn metadata_revalidation_retains_loaded_history_range_and_applies_latest_updates(
        cx: &mut TestAppContext,
    ) {
        cx.update(initialize);
        let (requests, _outgoing) = futures::channel::mpsc::unbounded();
        let (_events, events) = futures::channel::mpsc::unbounded();
        let window = cx.add_window(|window, cx| {
            WorkspaceView::new(Transport { requests, events }, window, cx)
        });
        window.update(cx, |view, window, cx| {
            view.cloud.account = Some("owner".into());
            view.cloud.namespace = Some("http://fixture.invalid".into());
            view.cloud.target = Some(json!({"workspaceId":"workspace","chatId":"thread","initialSessionId":"initial","state":"paused"}));
            view.cloud.status = json!({"state":"paused"});
            let entry = |id: &str, text: &str| json!({"id":id,"role":"assistant","status":"complete","parts":[{"id":id,"kind":"text","text":text}],"createdAt":1,"deviceId":"runtime"});
            let mut latest = history();
            latest["head"]["olderMessageSequence"] = json!(100);
            latest["head"]["transcript"]["reset"] = json!([entry("latest", "original")]);
            view.cloud.head = Some(serde_json::from_value(latest["head"].clone()).unwrap());
            view.cached_cloud_view(window, cx);
            let child = view.cloud.child.clone().unwrap();
            let child_id = child.entity_id();
            child.update(cx, |child, cx| child.input.update(cx, |input, cx| input.set_value("Keep this draft".into(), cx)));
            let mut older = latest.clone();
            older["historyPage"] = json!(true);
            older["requestedBefore"] = json!(100);
            older["head"]["olderMessageSequence"] = json!(99);
            older["head"]["transcript"]["reset"] = json!([entry("older", "retained")]);
            view.cloud.history_request = Some(930);
            assert!(view.receive_cloud(&json!({"id":930,"ok":older}), cx));
            view.prepare_cloud_workspace(window, cx);
            latest["head"]["cursor"]["version"] = json!(2);
            latest["head"]["transcript"]["reset"] = json!([entry("latest", "updated"),entry("new", "new response")]);
            view.cloud.history_request = Some(931);
            assert!(view.receive_cloud(&json!({"id":931,"ok":latest.clone()}), cx));
            view.prepare_cloud_workspace(window, cx);
            assert_eq!(view.cloud.child.as_ref().unwrap().entity_id(), child_id);
            assert_eq!(child.read(cx).messages.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), vec!["older", "latest", "new"], "A bounded latest-page refresh must retain the already loaded prefix");
            assert_eq!(view.cloud.head.as_ref().unwrap().older_message_sequence, Some(99));
            assert!(serde_json::to_string(&child.read(cx).messages).unwrap().contains("updated"), "Latest tool/text state remains authoritative");
            // Projection reset must discard retained data rather than mixing epochs.
            latest["head"]["cursor"]["epoch"] = json!("replacement-epoch");
            view.cloud.history_request = Some(932);
            assert!(view.receive_cloud(&json!({"id":932,"ok":latest}), cx));
            view.prepare_cloud_workspace(window, cx);
            assert_eq!(child.read(cx).messages.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), vec!["latest", "new"]);
            assert_eq!(view.cloud.head.as_ref().unwrap().older_message_sequence, Some(100));
        }).unwrap();
        window
            .update(cx, |view, window, _| {
                view.close_cloud();
                window.remove_window();
            })
            .unwrap();
        cx.run_until_parked();
    }
    #[gpui::test]
    fn cached_older_pages_merge_without_downgrading_a_later_live_bridge(cx: &mut TestAppContext) {
        cx.update(initialize);
        let (requests, _outgoing) = futures::channel::mpsc::unbounded();
        let (_events, events) = futures::channel::mpsc::unbounded();
        let window = cx.add_window(|window, cx| {
            WorkspaceView::new(Transport { requests, events }, window, cx)
        });
        window.update(cx, |view, window, cx| {
            view.cloud.account = Some("owner".into());
            view.cloud.target = Some(json!({"workspaceId":"workspace","chatId":"thread",
                "initialSessionId":"initial","state":"paused"}));
            view.cloud.status = json!({"state":"paused"});
            let entry = |id: &str| json!({"id":id,"role":"assistant","parts":[{"id":id,"kind":"text","text":id}],"createdAt":1,"deviceId":"runtime"});
            let mut latest = history();
            latest["head"]["olderMessageSequence"] = json!(100);
            latest["head"]["transcript"]["reset"] = json!([entry("latest")]);
            view.cloud.head = Some(serde_json::from_value(latest["head"].clone()).unwrap());
            view.cached_cloud_view(window, cx);
            let cached = view.cloud.child.clone().unwrap();
            let cached_id = cached.entity_id();
            cached.update(cx, |child, cx| {
                child.cloud.older_loading = true;
                child.input.update(cx, |input, cx| input.set_value("Keep draft".into(), cx));
            });
        }).unwrap();
        cx.run_until_parked();
        window.update(cx, |view, window, cx| {
            let cached = view.cloud.child.clone().unwrap();
            let cached_id = cached.entity_id();
            let entry = |id: &str| json!({"id":id,"role":"assistant","parts":[{"id":id,"kind":"text","text":id}],"createdAt":1,"deviceId":"runtime"});
            let mut older = history();
            older["historyPage"] = json!(true);
            older["requestedBefore"] = json!(100);
            older["head"]["olderMessageSequence"] = json!(50);
            older["head"]["transcript"]["reset"] = json!([entry("older")]);
            view.cloud.history_request = Some(920);
            assert!(view.receive_cloud(&json!({"id":920,"ok":older}), cx));
            view.prepare_cloud_workspace(window, cx);
            assert_eq!(view.cloud.child.as_ref().unwrap().entity_id(), cached_id);
            assert_eq!(cached.read(cx).messages.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), vec!["older","latest"]);
            assert_eq!(cached.read(cx).cloud.older_before, Some(50));
            assert!(!cached.read(cx).cloud.older_loading);
            assert_eq!(cached.read(cx).input.read(cx).value(cx), "Keep draft");
            assert!(!cached.read(cx).requests.is_closed());
            // The next page was requested in cached mode, but arrives after
            // the live document has taken ownership of its message indices.
            view.cloud.target.as_mut().unwrap()["state"] = json!("ready");
            view.cloud.status = json!({"state":"ready","epoch":1});
            view.prepare_cloud_workspace(window, cx);
            let live = view.cloud.child.clone().unwrap();
            let live_id = live.entity_id();
            assert_eq!(live_id,cached_id,"passive Ready promotes the existing cached child");
            live.update(cx, |child, _| child.messages=serde_json::from_value(json!([entry("live-newer")])).unwrap());
            older["requestedBefore"] = json!(50);
            older["head"]["olderMessageSequence"] = json!(25);
            older["head"]["transcript"]["reset"] = json!([entry("oldest")]);
            view.cloud.history_request = Some(921);
            assert!(view.receive_cloud(&json!({"id":921,"ok":older}), cx));
            view.prepare_cloud_workspace(window, cx);
            view.cached_cloud_view(window, cx);
            assert_eq!(view.cloud.child.as_ref().unwrap().entity_id(),live_id);
            assert!(!live.read(cx).cloud.cached);
            assert_eq!(live.read(cx).messages[0].id,"live-newer");
            assert_eq!(view.cloud.head.as_ref().unwrap().older_message_sequence,Some(25),"Merged history remains available for the next saved-history view");
        }).unwrap();
        window
            .update(cx, |view, window, _| {
                view.close_cloud();
                window.remove_window();
            })
            .unwrap();
        cx.run_until_parked();
    }

    #[gpui::test]
    fn admitted_cached_run_opens_passive_stream_and_preserves_same_child_transcript_and_draft(
        cx: &mut TestAppContext,
    ) {
        cx.update(initialize);
        let (requests, mut outgoing) = futures::channel::mpsc::unbounded();
        let (_events, incoming) = futures::channel::mpsc::unbounded();
        let window = cx.add_window(|window, cx| {
            WorkspaceView::new(
                Transport {
                    requests,
                    events: incoming,
                },
                window,
                cx,
            )
        });
        let (child_id, old_status) = window.update(cx,|view,window,cx| {
            view.connected=true; view.cloud.account=Some("owner".into());
            view.cloud.organization=Some("org-a".into());
            view.cloud.namespace=Some("https://cloud.example/".into());
            view.cloud.target=Some(json!({"workspaceId":"workspace","chatId":"thread","initialSessionId":"initial","state":"paused","commandDeliveryMode":"server-admitted-v2","commandAdmissionVersion":2}));
            view.cloud.head=Some(serde_json::from_value(history()["head"].clone()).unwrap());
            view.cloud.status=json!({"state":"paused","epoch":1});
            view.prepare_cloud_workspace(window,cx);
            let child=view.cloud.child.clone().unwrap();
            child.update(cx,|child,cx| {
                child.selected=Some(json!({"id":"initial","deviceId":"runtime","cwd":"."}));
                child.device_id=Some("runtime".into());
                child.messages=serde_json::from_value(json!([{"id":"retained","role":"assistant","parts":[{"id":"old-text","kind":"text","text":"Retained answer"}],"createdAt":1,"deviceId":"runtime"}])).unwrap();
                child.input.update(cx,|input,cx|input.set_value("Unsent second draft".into(),cx));
                child.open_file=Some(json!({"path":"draft.txt","text":"Unsaved editor content"}));
                child.transcript_scroll.set_offset(point(px(0.),px(-51.)));
            });
            let stale=view.call(methods::CLOUD_ENVIRONMENT,json!({"workspaceId":"workspace","expectedAccountId":"owner","expectedOrganizationId":"org-a","interactiveResume":false}));
            view.cloud.status_request=Some(stale);
            (child.entity_id(),stale)
        }).unwrap();
        cx.run_until_parked();
        while outgoing.try_recv().is_ok() {}
        let hint = json!({"mode":"passive","accountId":"owner","organizationId":"org-a","workspaceId":"workspace","sessionId":"initial","commandId":"admitted-once"});
        window.update(cx,|view,window,cx| {
            view.cloud.forwards.insert(700,(701,methods::QUEUE_COMMAND.into()));
            assert!(view.receive_cloud(&json!({"id":700,"item":{"id":701,"ok":{"durablyAdmitted":true,"commandId":"admitted-once","passiveAttachment":hint}}}),cx));
            let fresh=view.cloud.status_request.expect("admitted Run must own a passive status subscription");
            assert_ne!(fresh,old_status,"retired paused subscription must be replaced");
            assert!(view.receive_cloud(&json!({"id":fresh,"item":{"state":"ready","epoch":2}}),cx));
            view.prepare_cloud_workspace(window,cx);
            let child=view.cloud.child.as_ref().unwrap();
            assert_eq!(child.entity_id(),child_id,"Ready must promote the same cached child");
            let child=child.read(cx);
            assert_eq!(child.messages[0].id,"retained");
            assert_eq!(child.input.read(cx).value(cx),"Unsent second draft");
            assert_eq!(child.open_file.as_ref().unwrap()["text"],"Unsaved editor content");
            assert!(!child.cloud.cached,"same child becomes a live bridge");
        }).unwrap();
        cx.run_until_parked();
        window
            .update(cx, |view, _, cx| {
                let child = view.cloud.child.as_ref().unwrap();
                assert_eq!(child.entity_id(), child_id);
                let child = child.read(cx);
                assert_eq!(child.messages[0].id, "retained");
                assert_eq!(child.input.read(cx).value(cx), "Unsent second draft");
                assert_eq!(
                    child.open_file.as_ref().unwrap()["text"],
                    "Unsaved editor content"
                );
                assert!(
                    !child.cloud.cached,
                    "queued Connected event must retain the promoted view"
                );
            })
            .unwrap();
        let mut attaches = vec![];
        while let Ok(frame) = outgoing.try_recv() {
            if frame["method"] == methods::CLOUD_ENVIRONMENT {
                attaches.push(frame.clone());
            }
            assert_ne!(frame["ui"], "resumeCloud");
        }
        assert_eq!(attaches.len(), 1);
        assert_eq!(attaches[0]["params"]["interactiveResume"], false);
        assert_eq!(attaches[0]["params"]["expectedAccountId"], "owner");
        assert_eq!(attaches[0]["params"]["expectedOrganizationId"], "org-a");
        window
            .update(cx, |view, window, _| {
                view.close_cloud();
                window.remove_window();
            })
            .unwrap();
        cx.run_until_parked();
    }

    #[gpui::test]
    fn passive_admission_attachment_rejects_other_scope_and_is_noop_when_already_live(
        cx: &mut TestAppContext,
    ) {
        cx.update(initialize);
        let (requests, mut outgoing) = futures::channel::mpsc::unbounded();
        let (_events, incoming) = futures::channel::mpsc::unbounded();
        let window = cx.add_window(|window, cx| {
            WorkspaceView::new(
                Transport {
                    requests,
                    events: incoming,
                },
                window,
                cx,
            )
        });
        window.update(cx,|view,window,cx| {
            view.cloud.account=Some("owner".into());view.cloud.organization=Some("org-a".into());
            view.cloud.target=Some(json!({"workspaceId":"workspace","chatId":"thread","initialSessionId":"initial"}));
            view.cloud.head=Some(serde_json::from_value(history()["head"].clone()).unwrap());view.cloud.status=json!({"state":"paused"});
            view.prepare_cloud_workspace(window,cx);
            let valid=json!({"durablyAdmitted":true,"commandId":"once","passiveAttachment":{"mode":"passive","accountId":"owner","organizationId":"org-a","workspaceId":"workspace","sessionId":"initial","commandId":"once"}});
            for (pointer,value) in [
                ("/durablyAdmitted",json!(false)),("/passiveAttachment/accountId",json!("other")),
                ("/passiveAttachment/organizationId",json!("org-b")),("/passiveAttachment/workspaceId",json!("other")),
                ("/passiveAttachment/sessionId",json!("other")),("/passiveAttachment/commandId",json!("other")),
            ] {
                let mut changed=valid.clone();*changed.pointer_mut(pointer).unwrap()=value;
                view.attach_admitted_cloud_run(&changed,cx);assert!(view.cloud.status_request.is_none(),"scope fence {pointer}");
            }
            let (events,_)=futures::channel::mpsc::unbounded();view.cloud.events=Some(events);view.cloud.status=json!({"state":"ready","epoch":3});
            view.attach_admitted_cloud_run(&valid,cx);assert!(view.cloud.status_request.is_none(),"already live cannot reopen");
            view.cloud.status=json!({"state":"paused"});
            for (i,method)in [methods::QUEUE_MESSAGE,methods::UPDATE_QUEUED_MESSAGE,methods::WATCH_DOC_MESSAGES].into_iter().enumerate(){
                let id=900+i as u64;view.cloud.forwards.insert(id,(id+10,method.into()));
                assert!(view.receive_cloud(&json!({"id":id,"item":{"id":id+10,"ok":valid}}),cx));
                assert!(view.cloud.status_request.is_none(),"background/history never attach");
            }
        }).unwrap();
        cx.run_until_parked();
        while let Ok(frame) = outgoing.try_recv() {
            assert_ne!(frame["method"], methods::CLOUD_ENVIRONMENT);
        }
        window
            .update(cx, |view, window, _| {
                view.close_cloud();
                window.remove_window();
            })
            .unwrap();
        cx.run_until_parked();
    }

    #[gpui::test]
    fn explicit_sleep_retains_child_request_pump_for_later_admitted_send(cx: &mut TestAppContext) {
        cx.update(initialize);
        let (requests, mut outgoing) = futures::channel::mpsc::unbounded();
        let (_events, incoming) = futures::channel::mpsc::unbounded();
        let window = cx.add_window(|window, cx| {
            WorkspaceView::new(
                Transport {
                    requests,
                    events: incoming,
                },
                window,
                cx,
            )
        });
        let identity=window.update(cx,|view,window,cx| {
            view.connected=true;view.cloud.account=Some("owner".into());
            view.cloud.target=Some(json!({"workspaceId":"workspace","chatId":"thread","initialSessionId":"initial","state":"ready"}));
            view.cloud.head=Some(serde_json::from_value(history()["head"].clone()).unwrap());view.cloud.status=json!({"state":"ready","epoch":1});view.prepare_cloud_workspace(window,cx);
            let child=view.cloud.child.clone().unwrap();let identity=child.entity_id();assert!(view.cloud.pump.is_some());
            child.update(cx,|child,cx|{child.selected=Some(json!({"id":"initial","deviceId":"runtime","cwd":"."}));child.input.update(cx,|input,cx|input.set_value("Preserve draft".into(),cx));});
            identity
        }).unwrap();
        // TextInput::set_value commits through cx.defer/update_window. Settle
        // the initial Connected event and the actual editor write before Sleep.
        cx.run_until_parked();
        window.update(cx,|view,window,cx| {
            let child=view.cloud.child.clone().unwrap();
            assert_eq!(child.entity_id(),identity);
            assert_eq!(child.read(cx).input.read(cx).value(cx),"Preserve draft","positive pre-Sleep editor state");
            view.cloud.pause=Some(810);
            assert!(view.receive_cloud(&json!({"id":810,"ok":{"workspace":{"workspaceId":"workspace","chatId":"thread","initialSessionId":"initial","state":"paused"}}}),cx));
            view.prepare_cloud_workspace(window,cx);
            assert_eq!(view.cloud.child.as_ref().unwrap().entity_id(),identity);assert!(view.cloud.pump.is_some(),"Sleep must retain the receiver for the same child");assert!(view.cloud.cached_events.is_some());
            assert!(!child.read(cx).requests.is_closed());assert_eq!(child.read(cx).input.read(cx).value(cx),"Preserve draft");
            child.read(cx).requests.unbounded_send(json!({"id":991,"method":methods::QUEUE_MESSAGE,"params":{"chatId":"initial","text":"queue without wake"}})).unwrap();
            view.attach_admitted_cloud_run(&json!({"durablyAdmitted":true,"commandId":"after-sleep","passiveAttachment":{"mode":"passive","accountId":"owner","organizationId":null,"workspaceId":"workspace","sessionId":"initial","commandId":"after-sleep"}}),cx);
            assert!(view.cloud.status_request.is_some());
        }).unwrap();
        cx.run_until_parked();
        window
            .update(cx, |view, _, cx| {
                let child = view.cloud.child.as_ref().unwrap();
                assert_eq!(child.entity_id(), identity);
                assert!(!child.read(cx).requests.is_closed());
                assert_eq!(
                    child.read(cx).input.read(cx).value(cx),
                    "Preserve draft",
                    "queued connection/editor work must retain the draft"
                );
            })
            .unwrap();
        let mut attaches = 0;
        let mut queued = 0;
        while let Ok(frame) = outgoing.try_recv() {
            if frame["method"] == methods::CLOUD_ENVIRONMENT {
                attaches += 1;
                assert_eq!(frame["params"]["interactiveResume"], false);
            }
            if frame["method"] == methods::CLOUD_RPC
                && frame["params"]["method"] == methods::QUEUE_MESSAGE
            {
                queued += 1;
            }
        }
        assert_eq!(attaches, 1);
        assert_eq!(
            queued, 1,
            "same receiver routes a real queued request after Sleep"
        );
        window
            .update(cx, |view, window, _| {
                view.close_cloud();
                window.remove_window();
            })
            .unwrap();
        cx.run_until_parked();
    }
}
