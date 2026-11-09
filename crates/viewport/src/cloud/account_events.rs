//! One logical account subscription; the host multiplexes one physical feed per
//! actor/API/organization. This path reads metadata and never wakes computers.
use super::*;
#[derive(Default)]
pub(super) struct State {
    pub request: Option<u64>,
    pub live: bool,
    scope: Option<String>,
    dirty: bool,
    history_dirty: bool,
    pub draft: Option<(String, Vec<Value>)>,
}
impl WorkspaceView {
    pub(super) fn watch_cloud_account_events(&mut self) {
        if self.cloud.guest || self.cached_only {
            return;
        }
        let scope = self.cloud.account.as_ref().map(|account| {
            json!([self.cloud.namespace, account, self.cloud.organization]).to_string()
        });
        if self.cloud.account_events.scope != scope {
            if let Some(id) = self.cloud.account_events.request.take() {
                self.cancel(id);
                self.pending.remove(&id);
            }
            self.cloud.account_events = State {
                scope: scope.clone(),
                ..Default::default()
            };
        }
        if scope.is_some() && self.cloud.account_events.request.is_none() {
            self.cloud.account_events.request=Some(self.call(methods::CLOUD_EVENTS,json!({"expectedAccountId":self.cloud.account,"expectedOrganizationId":self.cloud.organization})));
        }
    }
    pub(super) fn flush_cloud_account_events(&mut self) {
        if self.cloud.account_events.history_dirty && self.cloud.history_request.is_none() {
            if let (Some(target), Some(head)) = (&self.cloud.target, &self.cloud.head) {
                let session = &head.session_id;
                let body = json!({"workspaceId":target["workspaceId"],"sessionId":session,"incremental":true,"expectedAccountId":self.cloud.account,"expectedOrganizationId":self.cloud.organization});
                self.cloud.account_events.history_dirty = false;
                self.cloud.history_request = Some(self.call(methods::CLOUD_HISTORY, body));
            }
        }
        if self.cloud.account_events.dirty
            && self.cloud.context.is_none()
            && self.cloud.list.is_none()
        {
            self.cloud.account_events.dirty = false;
            self.refresh_cloud();
        }
    }
    pub(super) fn restore_cloud_event_draft(&mut self, cx: &mut Context<Self>) {
        if let Some(child) = &self.cloud.child {
            if let Some((text, attachments)) = self.cloud.account_events.draft.take() {
                child.update(cx, |view, cx| {
                    view.input.update(cx, |input, cx| input.set_value(text, cx));
                    view.attachments = attachments;
                    cx.notify();
                });
            }
        }
    }
    pub(super) fn receive_cloud_account_events(
        &mut self,
        frame: &Value,
        cx: &mut Context<Self>,
    ) -> bool {
        if frame["id"].as_u64() != self.cloud.account_events.request {
            return false;
        }
        if frame["id"].as_u64().is_none() {
            return false;
        }
        if let Some(value) = frame.get("item").or_else(|| frame.get("ok")) {
            if value["accountId"].as_str() != self.cloud.account.as_deref()
                || value["organizationId"].as_str() != self.cloud.organization.as_deref()
            {
                return true;
            }
            let data = &value["data"];
            let kind = data["type"].as_str().unwrap_or_default();
            let revoked = data["revokedWorkspaceIds"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>();
            // Lost delivery cursors invalidate cached pages, not account access.
            // Keep the visible child (and its live stream, draft and scroll) while
            // an independent history read revalidates the same authorized task.
            let cache_invalidated = data["cacheRevoked"] == true;
            let replay_reset = kind == "reset"
                && matches!(
                    data["reason"].as_str(),
                    None | Some("cursor_expired" | "consumer_lagged")
                );
            let revoke_all = cache_invalidated && !replay_reset;
            self.cloud.heads.retain(|_, (head, _)| {
                !cache_invalidated && !revoked.contains(&head.workspace_id.as_str())
            });
            if cache_invalidated && replay_reset {
                self.cloud.head_verified = None;
                if let Some(id) = self.cloud.cache_request.take() {
                    self.cancel(id);
                    self.pending.remove(&id);
                }
                self.revoke_cloud_archive_history();
            }
            let target_revoked = self
                .cloud
                .target
                .as_ref()
                .and_then(|target| target["workspaceId"].as_str())
                .is_some_and(|id| revoked.contains(&id));
            if revoke_all || target_revoked {
                let target = (!target_revoked && !matches!(kind, "error" | "closed"))
                    .then(|| self.cloud.target.clone())
                    .flatten();
                let draft = target
                    .as_ref()
                    .and_then(|_| self.cloud.child.as_ref())
                    .map(|child| {
                        let view = child.read(cx);
                        (view.input.read(cx).value(cx), view.attachments.clone())
                    });
                self.revoke_cloud_archive_history();
                self.close_cloud();
                self.cloud.heads.clear();
                if let Some(target) = target {
                    self.open_cloud(target);
                    self.cloud.account_events.draft = draft;
                }
            } else if !revoked.is_empty() {
                if self
                    .cloud_archive
                    .state
                    .head
                    .as_ref()
                    .is_some_and(|head| revoked.contains(&head.workspace_id.as_str()))
                {
                    self.revoke_cloud_archive_history();
                }
            }
            match kind {
                "connected" => {
                    self.cloud.account_events.live = true;
                    self.cloud.account_events.dirty = true;
                    self.cloud.poll = None;
                    self.cloud.sync.poll = None;
                    self.cloud.repair.poll = None;
                }
                "reset" | "revalidate" => {
                    self.cloud.account_events.dirty = true;
                    // A resumed cursor may have lost conversation invalidations.
                    // Revalidate the visible saved head through account HTTP;
                    // retain its existing child/scroll while the read completes.
                    if self.cloud.target.is_some() && self.cloud.head.is_some() {
                        self.cloud.account_events.history_dirty = true;
                    }
                }
                "events" => {
                    for event in data["events"].as_array().into_iter().flatten() {
                        if matches!(
                            event["kind"].as_str(),
                            Some("task-command-queue" | "task-command-progress" | "conversation")
                        ) && self.cloud.target.as_ref().is_some_and(|target| {
                            target["commandDeliveryMode"] == "server-admitted-v2"
                                && (event["workspaceId"] == target["workspaceId"]
                                    || event["resourceId"] == target["workspaceId"])
                        }) {
                            self.cloud.account_events.history_dirty = true;
                        }
                        if event["kind"] == "workspace"
                            && event["payload"]["recreation"] == true
                            && self.cloud.target.as_ref().is_some_and(|target| {
                                event["workspaceId"] == target["workspaceId"]
                                    || event["resourceId"] == target["workspaceId"]
                            })
                        {
                            self.refresh_cloud_recreation();
                        }
                        if event["kind"] == "workspace"
                            && event["payload"]["runtimeRepair"] == true
                            && self.cloud.target.as_ref().is_some_and(|target| {
                                event["workspaceId"] == target["workspaceId"]
                                    || event["resourceId"] == target["workspaceId"]
                            })
                        {
                            self.refresh_cloud_runtime_repair();
                            if self.cloud.sync.metadata.is_none() {
                                if let Some(target) = &self.cloud.target {
                                    let body = json!({"workspaceId":target["workspaceId"],"expectedAccountId":self.cloud.account,"expectedOrganizationId":self.cloud.organization});
                                    self.cloud.sync.metadata =
                                        Some(self.call(methods::CLOUD_WORKSPACE, body));
                                }
                            }
                        }
                        match event["kind"].as_str() {
                            Some(
                                "workspace" | "conversation" | "connection" | "github"
                                | "environment",
                            ) => self.cloud.account_events.dirty = true,
                            Some("authentication") => {
                                self.cloud.account_events.dirty = true;
                                self.repository_panel.update(cx, |panel, cx| {
                                    panel.account_event(
                                        "authentication",
                                        event["resourceId"].as_str().unwrap_or_default(),
                                        cx,
                                    )
                                });
                            }
                            _ => {}
                        }
                        if self.settings
                            && self.settings_section == crate::settings_view::Section::Cloud
                        {
                            if let Some(kind) = event["kind"]
                                .as_str()
                                .filter(|kind| *kind != "authentication")
                            {
                                let resource = event["resourceId"].as_str().unwrap_or_default();
                                self.repository_panel.update(cx, |panel, cx| {
                                    panel.account_event(kind, resource, cx)
                                });
                            }
                        }
                        if event["kind"] == "workspace" && self.cloud.sync.metadata.is_none() {
                            if let Some(target) = self.cloud.target.as_ref().filter(|target| {
                                target["workspaceId"] == event["workspaceId"]
                                    || target["workspaceId"] == event["resourceId"]
                            }) {
                                let body = json!({"workspaceId":target["workspaceId"],"expectedAccountId":self.cloud.account,"expectedOrganizationId":self.cloud.organization});
                                self.cloud.sync.metadata =
                                    Some(self.call(methods::CLOUD_WORKSPACE, body));
                            }
                        }
                    }
                }
                "disabled" | "reconnecting" | "closed" | "error" => {
                    self.cloud.account_events.live = false
                }
                _ => {}
            }
            let live = self.cloud.account_events.live;
            self.repository_panel
                .update(cx, |panel, cx| panel.set_account_feed_live(live, cx));
            self.flush_cloud_account_events();
            cx.notify();
        }
        if frame["done"] == true || frame.get("err").is_some() {
            if let Some(id) = self.cloud.account_events.request.take() {
                self.pending.remove(&id);
            }
            self.cloud.account_events.live = false;
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;
    use std::prelude::v1::test;

    fn saved_head() -> zuse_model::cloud::Head {
        serde_json::from_value(json!({"schemaVersion":1,"payloadFormat":"zuse-cloud-admission-v1",
            "workspaceId":"workspace","sessionId":"session","runtimeGeneration":1,
            "cursor":{"epoch":"epoch","version":7},
            "chat":{"id":"session","deviceId":"runtime","archived":false,"createdAt":"2026-01-01T00:00:00Z"},
            "conversations":[],"transcript":{"reset":[]},"queue":[],"queuePaused":false,
            "olderMessageSequence":40})).unwrap()
    }

    #[gpui::test]
    fn replay_reset_revalidates_history_without_replacing_conversation(cx: &mut TestAppContext) {
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
        let child_id = window.update(cx, |view, window, cx| {
            let (requests, _child_outgoing) = futures::channel::mpsc::unbounded();
            let (_child_events, incoming) = futures::channel::mpsc::unbounded();
            let child = cx.new(|cx| WorkspaceView::new(Transport { requests, events: incoming }, window, cx));
            child.update(cx, |child, cx| child.input.update(cx, |input, cx| input.set_value("Unsent draft".into(), cx)));
            let child_id = child.entity_id();
            view.cloud.child = Some(child);
            view.cloud.account = Some("owner".into());
            view.cloud.organization = Some("org".into());
            view.cloud.target = Some(json!({"workspaceId":"workspace","initialSessionId":"session","state":"paused"}));
            view.cloud.head = Some(saved_head());
            view.cloud.head_verified = Some(web_time::Instant::now());
            view.cloud.heads.insert("workspace".into(), (saved_head(), web_time::Instant::now()));
            view.cloud.context = Some(9000);
            view.cloud.list = Some(9001);
            view.cloud.account_events.request = Some(90);
            child_id
        }).unwrap();
        // Input::set_value deliberately defers editor writes to its window.
        // Establish the real draft before testing the event, rather than reading
        // the constructor's still-empty editor in the same update closure.
        cx.run_until_parked();
        window
            .update(cx, |view, _, cx| {
                assert_eq!(
                    view.cloud
                        .child
                        .as_ref()
                        .unwrap()
                        .read(cx)
                        .input
                        .read(cx)
                        .value(cx),
                    "Unsent draft"
                );
                while outgoing.try_recv().is_ok() {}
                for (index, reason) in [
                    Value::Null,
                    json!("cursor_expired"),
                    json!("consumer_lagged"),
                ]
                .into_iter()
                .enumerate()
                {
                    assert!(view.receive_cloud_account_events(
                        &json!({"id":90,"item":{"accountId":"owner","organizationId":"org",
                    "data":{"type":"reset","cacheRevoked":true,"reason":reason}}}),
                        cx
                    ));
                    assert_eq!(view.cloud.child.as_ref().unwrap().entity_id(), child_id);
                    let child = view.cloud.child.as_ref().unwrap().read(cx);
                    assert_eq!(child.input.read(cx).value(cx), "Unsent draft");
                    assert_eq!(
                        view.cloud.head.as_ref().unwrap().older_message_sequence,
                        Some(40)
                    );
                    assert!(view.cloud.heads.is_empty());
                    assert!(view.cloud.head_verified.is_none());
                    assert!(view.cloud.history_request.is_some());
                    let mut reads = 0;
                    while let Ok(frame) = outgoing.try_recv() {
                        if frame.get("method").is_some() {
                            assert_eq!(
                                frame["method"],
                                methods::CLOUD_HISTORY,
                                "cursor reset must not attach, resume or send"
                            );
                            assert_eq!(frame["params"]["workspaceId"], "workspace");
                            assert_eq!(frame["params"]["expectedOrganizationId"], "org");
                            reads += 1;
                        }
                    }
                    assert_eq!(
                        reads,
                        if index == 0 { 1 } else { 0 },
                        "pending history reads coalesce resets"
                    );
                }
                assert!(view.receive_cloud_account_events(
                    &json!({"id":90,"item":{"accountId":"owner","organizationId":"org",
                "data":{"type":"events","revokedWorkspaceIds":["workspace"]}}}),
                    cx
                ));
                assert!(
                    view.cloud.child.is_none(),
                    "explicit target revocation still removes the view"
                );
                assert!(view.cloud.head.is_none());
            })
            .unwrap();
    }
    #[gpui::test]
    fn history_permission_errors_remove_visible_rows_but_transport_failure_keeps_view(
        cx: &mut TestAppContext,
    ) {
        cx.update(initialize);
        let (requests, _outgoing) = futures::channel::mpsc::unbounded();
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
        window.update(cx, |view, window, cx| {
            for error in [
                "Cloud service is unavailable. Try again.",
                "transport request timed out after 401 ms",
                "capability error: workspace_not_found",
                "tool_output_not_published",
                "projection_not_found",
                "You do not have access to this workspace.",
                "Cloud could not verify your session. Reconnect in Settings → General.",
            ] {
                let (requests, _child_outgoing) = futures::channel::mpsc::unbounded();
                let (_child_events, incoming) = futures::channel::mpsc::unbounded();
                let child = cx.new(|cx| WorkspaceView::new(Transport { requests, events: incoming }, window, cx));
                let child_id = child.entity_id();
                view.error = None;
                view.cloud.child = Some(child);
                view.cloud.account = Some("owner".into());
                view.cloud.target = Some(json!({"workspaceId":"workspace","initialSessionId":"session","state":"paused"}));
                view.cloud.head = Some(saved_head());
                let id = view.call(methods::CLOUD_HISTORY, json!({"workspaceId":"workspace"}));
                view.cloud.history_request = Some(id);
                assert!(view.receive_cloud(&json!({"id":id,"err":error}), cx));
                if error.starts_with("Cloud service is unavailable") || error.starts_with("transport ") || matches!(error, "tool_output_not_published" | "projection_not_found") {
                    assert_eq!(view.cloud.child.as_ref().unwrap().entity_id(), child_id);
                    assert!(view.cloud.head.is_some());
                } else {
                    assert!(view.cloud.child.is_none(), "authoritative denial must clear visible cached content");
                    assert!(view.cloud.head.is_none());
                    assert_eq!(view.error.as_deref(), Some(if error == "capability error: workspace_not_found" {
                        "This conversation is no longer available. Choose another conversation or start a new chat."
                    } else {
                        "Sign in with the account that has access to this workspace, then reopen the conversation."
                    }), "the recovery message must survive closing cloud mode");
                    assert!(view.has_composer_notices(), "global composer must render the denial recovery notice");
                }
            }
        }).unwrap();
    }
}
