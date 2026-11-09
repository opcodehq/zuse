//! Rebuild read subscriptions without replaying actions with uncertain outcomes.
use super::*;
use crate::localization::message as tr;
use crate::shell::{column, control, label, row};
use zuse_i18n::messages as msg;
use zuse_protocol::browser_link::InterruptedRequest;
const HOSTED_CONTROLLER_BUSY: &str = "Zuse is busy. Close an unused browser tab and retry shortly.";

impl WorkspaceView {
    pub(super) fn require_connection(&mut self, cx: &mut Context<Self>) -> bool {
        if self.cloud.cached {
            let _ = self.requests.unbounded_send(json!({"ui":"resumeCloud"}));
            return false;
        }
        if self.cached_only {
            return false;
        }
        if self.connected {
            true
        } else {
            self.error = Some(
                "Reconnect to the engine before performing this action. Your draft is preserved."
                    .into(),
            );
            cx.notify();
            false
        }
    }
    pub(super) fn receive_connection(&mut self, frame: &Value, cx: &mut Context<Self>) {
        if frame["controllerBusy"] == true {
            self.error = Some(HOSTED_CONTROLLER_BUSY.into());
        }
        if frame["hostedSessionRevoked"] == true {
            // A new anonymous cloud controller cannot inherit the previous
            // actor's protected history, drafts, or public controller identity.
            self.receive_cloud(&json!({"cacheRevoked":true}), cx);
            self.cloud = Default::default();
            self.cloud.mode = self.cloud_controller;
            self.cloud_account = Value::Null;
            self.drafts.clear();
            self.device_id = None;
            self.attachments.clear();
            self.send_draft.clear();
            self.uncertain_drafts.clear();
            self.input
                .update(cx, |input, cx| input.set_value(String::new(), cx));
            self.error = Some("Sign in to Zuse to continue.".into());
            let _ = self
                .requests
                .unbounded_send(json!({"ui":"hostedSessionRevoked"}));
        }
        if frame["connection"] == "connected" {
            if self.error.as_deref() == Some(HOSTED_CONTROLLER_BUSY) {
                self.error = None;
            }
            self.connection_epoch = frame["epoch"].as_u64().unwrap_or(0);
            self.connected = false;
            self.reset_browser(cx);
            self.reset_visuals();
            self.disconnect_browser_voice(cx);
            self.workspace_synced = false;
            self.status = "Verifying engine…".into();
            self.call(methods::LOCAL_DEVICE, json!({}));
            cx.notify();
            return;
        }
        let interrupted =
            serde_json::from_value::<Vec<InterruptedRequest>>(frame["interrupted"].clone())
                .unwrap_or_default();
        let sent: std::collections::HashSet<u64> =
            interrupted.iter().map(|request| request.id).collect();
        let draft_ids: std::collections::HashSet<u64> = self.send_draft.keys().copied().collect();
        let save_id = self.save_request.as_ref().map(|save| save.id);
        if interrupted.iter().any(|request| {
            request.mutation && !draft_ids.contains(&request.id) && Some(request.id) != save_id
        }) {
            self.connection_notice=Some("An action may have completed before the connection closed. Check its current state before trying it again.".into());
        }
        self.interrupt_project_actions(cx);
        self.terminal_open = None;
        self.terminal_closing.clear();
        self.interrupt_title_settings();
        self.interrupt_worktree_settings();
        self.interrupt_thread_action();
        self.interrupt_queue_edit(cx);
        self.interrupt_model_config(cx);
        self.provider_updates_watch = None;
        self.provider_updates_action = None;
        self.provider_updates_cancel = None;
        self.provider_updates_confirmed = false;
        self.connected = false;
        self.reset_browser(cx);
        self.reset_visuals();
        self.disconnect_browser_voice(cx);
        self.notification_baselines.clear();
        self.workspace_synced = false;
        self.status = if frame["connection"] == "pairingRequired" {
            self.error = frame["message"].as_str().map(str::to_owned);
            "Pairing invitation required · Open a new invitation".to_owned()
        } else if frame["connection"] == "authRequired" {
            "Authentication required · Open the engine access link".to_owned()
        } else if frame["connection"] == "accountRequired" {
            self.error = frame["message"].as_str().map(str::to_owned);
            "Account access required · Return to the controller".to_owned()
        } else if self.account_remote {
            "Reconnecting to account computer…".to_owned()
        } else if self.paired {
            "Reconnecting to paired engine…".to_owned()
        } else {
            crate::localization::message(cx, zuse_i18n::messages::native::reconnecting)
        }
        .into();
        if (self.paired || self.account_remote)
            && frame["connection"] != "accountRequired"
            && let Some(message) = frame["message"].as_str()
        {
            self.status = format!("Computer unavailable · {message}");
        }
        let had_pending = !self.pending.is_empty();
        for (id, mut draft) in std::mem::take(&mut self.send_draft) {
            draft.not_sent = frame.get("interrupted").is_some() && !sent.contains(&id);
            self.uncertain_drafts.push(draft);
        }
        if let Some(save) = self.save_request.take() {
            if sent.contains(&save.id) {
                self.uncertain_save = Some(save);
            }
        }
        if let Some(save) = self.access_save.take() {
            self.access_unconfirmed = Some(save.chat);
            self.connection_notice = Some("The permission change may have completed. The saved mode will be checked after reconnecting.".into());
        }
        self.creating = None;
        self.interrupt_custom_acp();
        self.interrupt_provider_logins(cx);
        self.archive_devices.clear();
        self.interrupt_model_catalogs();
        if !self.provider_actions.is_empty() {
            self.provider_error=Some("Connection interrupted. Refresh provider status and check linked accounts before trying the action again.".into());
            self.provider_actions.clear();
        }
        if let Some((_, chat)) = self.queue_pause_request.take() {
            self.queue_unconfirmed.insert(chat);
        }
        if let Some((_, chat, _)) = self.unknown_review_request.take() {
            self.queue_unconfirmed.insert(chat);
        }
        for (_, action) in self.queue_actions.drain() {
            self.queue_unconfirmed.insert(action.chat);
        }
        self.interrupt_cloud_archive();
        self.interrupt_ui_block_replies(cx);
        self.interrupt_cloud_transport(cx);
        self.pending.clear();
        self.stop_transcript();
        // This card is read-only: keep it usable while retrying the same host.
        self.context_confirmed = false;
        self.queue_watch = None;
        self.directory_request = None;
        self.file_request = None;
        self.file_read_baseline = None;
        self.file_reconcile = None;
        self.file_reconnect_needed = self.open_file.is_some();
        self.recovery_request = None;
        self.secret_text
            .update(cx, |input, cx| input.set_value(String::new(), cx));
        self.secret_editing = None;
        self.input_pending.clear();
        self.input_submitted.clear();
        self.interrupt_pull_requests();
        self.git_requests.clear();
        self.git_controls
            .update(cx, |panel, cx| panel.disconnected(cx));
        self.plugin_request = None;
        self.plugin_state.interrupted();
        self.plugin_setup = None;
        self.plugin_setup_events = None;
        self.interrupt_managed_access();
        self.pairing_fetch = None;
        self.remote_ui.interrupted();
        self.relay_fetch = None;
        self.relay_action = None;
        self.relay_status = Value::Null;
        self.direct_fetch = None;
        self.direct_action = None;
        self.direct_status = Value::Null;
        self.pairing_action = None;
        self.terminal_watch = None;
        self.terminal_write = None;
        self.interrupt_tool_results();
        self.interrupt_previews();
        self.interrupt_review();
        self.interrupt_history();
        if had_pending || self.uploading > 0 {
            self.composer_generation = self.composer_generation.wrapping_add(1);
            self.uploading = 0;
        }
        cx.notify();
    }
    pub(super) fn verify_engine(&mut self, value: &Value, cx: &mut Context<Self>) {
        let Some(device) = value["deviceId"].as_str() else {
            self.status = "Engine identity unavailable".into();
            return;
        };
        let cloud_controller = value["capabilities"]
            .as_array()
            .is_some_and(|caps| caps.iter().any(|v| v == "cloud-controller-v1"));
        if self
            .device_id
            .as_deref()
            .is_some_and(|previous| previous != device)
            && cloud_controller
        {
            self.receive_connection(
                &json!({"connection":"authRequired","hostedSessionRevoked":true}),
                cx,
            );
        }
        if self
            .device_id
            .as_deref()
            .is_some_and(|previous| previous != device)
        {
            self.status = "Engine identity changed · Reload when your local edits are safe".into();
            self.error=Some("This link now points to another engine. Your local draft and editor text have been preserved.".into());
            self.connected = false;
            self.reset_browser(cx);
            self.reset_visuals();
            self.disconnect_browser_voice(cx);
            return;
        }
        self.cloud_controller = cloud_controller;
        if cloud_controller {
            self.cloud.mode = true;
        }
        let first_identity = self.device_id.is_none();
        self.device_id = Some(device.into());
        if first_identity {
            let _ = self
                .requests
                .unbounded_send(json!({"ui":"loadDrafts","deviceId":device}));
        }
        self.connected = true;
        self.status = if self.cloud.guest {
            "Cloud workspace connected"
        } else if cloud_controller {
            "Connected to Zuse"
        } else if self.account_remote {
            "Account computer connected"
        } else if self.paired {
            "Paired engine connected"
        } else {
            "Local engine connected"
        }
        .into();
        self.zots.watches.clear();
        self.watch_zot_home(device.to_owned());
        self.call(methods::ENGINE_INFO, json!({}));
        for method in [
            methods::AUTH_STATUS,
            methods::CLOUD_ACCOUNT_STATUS,
            methods::LIST_HARNESSES,
            methods::WATCH_CHATS,
            methods::WATCH_SESSIONS,
            methods::WATCH_SPACES,
            methods::WATCH_DEVICES,
            methods::LIST_REPOS,
        ] {
            self.call(
                method,
                if matches!(method, methods::WATCH_CHATS | methods::WATCH_SESSIONS) {
                    json!({"delta":true})
                } else {
                    json!({})
                },
            );
        }
        self.prefetch_model_catalogs();
        self.refresh_model_config_watch();
        self.refresh_provider_updates();
        self.refresh_pairings();
        if self.settings && self.settings_section == settings_view::Section::Devices {
            self.refresh_managed_access();
        }
        self.refresh_worktree_settings();
        self.refresh_title_settings();
        if self.cloud_account["user"].is_object() {
            self.plugin_action(self.plugin_state.refresh());
        }
        if self.settings {
            self.call(methods::LIST_AGENT_ACCOUNTS, json!({"forceUsage":false}));
        }
        if let Some(chat) = self.selected.clone() {
            self.begin_transcript(cx);
            self.queue_watch = Some(self.call(
                methods::WATCH_QUEUE,
                zuse_protocol::presentation::chat_request(&chat, json!({})),
            ));
            self.load_directory(&self.directory.clone());
            if self.file_reconnect_needed {
                if let Some(file) = &self.open_file {
                    self.file_reconcile=Some(self.call(methods::READ_WORKSPACE_FILE,json!({"chatId":chat["id"],"path":file["path"],"targetDeviceId":chat["deviceId"]})));
                }
            }
            if self.right_open {
                match self.right_tab.as_str() {
                    "changes" => self.refresh_changes(cx),
                    "history" => self.refresh_history(cx),
                    "recovery" => self.inspect_recovery(),
                    "actions" => self.refresh_project_actions(),
                    _ => {}
                }
            }
        }
        if let Some(terminal) = &self.terminal_id {
            self.terminal_watch = Some(self.call(
                methods::SUBSCRIBE_TERMINAL,
                json!({"terminalId":terminal,"afterSeq":self.terminal_last_seq,"targetDeviceId":self.terminal_host}),
            ));
        }
        self.sync_previews(true, cx);
        self.restore_cloud_transport();
    }
    pub(super) fn receive_file_reconcile(
        &mut self,
        id: u64,
        frame: &Value,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.file_reconcile != Some(id) {
            return false;
        }
        if frame.get("ok").is_none() && frame.get("err").is_none() && frame["done"] != true {
            return true;
        }
        self.pending.remove(&id);
        self.file_reconcile = None;
        self.file_reconnect_needed = false;
        if let Some(error) = frame["err"].as_str() {
            self.error = Some(format!(
                "Could not verify the file after reconnecting: {error}. Your editor text is preserved."
            ));
            self.uncertain_save = None;
        } else if let Some(fresh) = frame.get("ok") {
            if let Some(old) = self.open_file.as_ref() {
                if fresh["path"] == old["path"] && fresh["checkoutId"] == old["checkoutId"] {
                    let editor = self.file_editor.read(cx).value(cx);
                    let unchanged = fresh["contentHash"] == old["contentHash"];
                    let committed = self.uncertain_save.as_ref().is_some_and(|save| {
                        fresh["text"].as_str() == Some(save.text.as_str())
                            && save.file["path"] == fresh["path"]
                            && save.file["checkoutId"] == fresh["checkoutId"]
                    });
                    let clean = old["text"].as_str() == Some(editor.as_str());
                    if committed || unchanged || clean {
                        if clean && !committed {
                            let text = fresh["text"].as_str().unwrap_or_default().to_owned();
                            self.file_editor
                                .update(cx, |input, cx| input.set_value(text, cx));
                        }
                        self.file_editor.update(cx, |input, cx| {
                            input.set_readonly(
                                fresh["readOnlyReason"].is_string() || fresh["truncated"] == true,
                                cx,
                            )
                        });
                        self.open_file = Some(fresh.clone());
                        if committed {
                            self.connection_notice=Some("The pending save is present on disk. Later editor changes remain unsaved.".into());
                        } else if self.uncertain_save.is_some() {
                            self.connection_notice=Some("The save was not confirmed. Your editor text is preserved; inspect the file before saving again.".into());
                        }
                    } else {
                        self.error=Some("The file changed while disconnected. Your editor text is preserved; reload or merge before saving.".into());
                    }
                } else {
                    self.error=Some("The file checkout changed while disconnected. Your editor text is preserved.".into());
                }
            }
            self.uncertain_save = None;
        }
        cx.notify();
        true
    }
    pub(super) fn confirm_deliveries(&mut self) {
        let chat = self.selected.as_ref().and_then(|chat| chat["id"].as_str());
        self.uncertain_drafts.retain(|draft| {
            !(draft.chat_id.as_deref() == chat
                && draft
                    .message_id
                    .as_ref()
                    .is_some_and(|id| self.messages.iter().any(|message| message.id == *id)))
        });
    }
    fn recover_draft(&mut self, index: usize, cx: &mut Context<Self>) {
        if self.queue_edit_navigation_blocked(cx) {
            return;
        }
        if !self.require_connection(cx) {
            return;
        }
        if !self.input.read(cx).value(cx).is_empty() || !self.attachments.is_empty() {
            self.error =
                Some("Keep or clear the current draft before restoring another one.".into());
            cx.notify();
            return;
        }
        let Some(draft) = self.uncertain_drafts.get(index).cloned() else {
            return;
        };
        let destination = self.drafts.get(&draft.key).cloned();
        if draft.chat_id.as_deref() != self.selected.as_ref().and_then(|chat| chat["id"].as_str()) {
            if self.file_navigation_blocked(cx) {
                return;
            }
            if let Some(chat) = self
                .chats
                .iter()
                .find(|chat| chat["id"].as_str() == draft.chat_id.as_deref())
                .cloned()
            {
                if !self.select(chat, cx) {
                    return;
                }
            } else if !self.new_chat(cx) {
                return;
            }
        }
        if !draft.attachments.is_empty() && draft.host_id != self.draft_host() {
            self.error = Some("These attachments belong to another computer. Reopen the original host or upload the files again before restoring this draft.".into());
            cx.notify();
            return;
        }
        if let Some(current) = destination {
            if !current.is_empty() && current != draft && !self.uncertain_drafts.contains(&current)
            {
                self.uncertain_drafts.push(current);
            }
        }
        self.uncertain_drafts.remove(index);
        let not_sent = draft.not_sent;
        self.attachments = draft.attachments;
        self.input
            .update(cx, |input, cx| input.set_value(draft.text, cx));
        self.connection_notice=Some(if not_sent {"Restored an unsent draft."}else{"Draft restored. Review the chat and queue before sending; its earlier delivery may have completed."}.into());
        self.sync_previews(true, cx);
        cx.notify();
    }
    pub(super) fn connection_recovery_view(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut view = column().gap_2();
        if let Some(error) = &self.draft_journal_error {
            view = view.child(row().gap_2()
                .child(label(format!("Browser draft storage: {error}. Keep this tab open or copy your text before closing.")).flex_1())
                .child(control("retry-draft-storage", tr(cx, msg::chat::attempts_retry)).on_click(cx.listener(|this, _, _, cx| this.retry_draft_storage(cx)))));
        }
        if let Some(notice) = &self.connection_notice {
            view = view.child(
                row()
                    .gap_2()
                    .px_3()
                    .py_2()
                    .border_b_1()
                    .border_color(rgb(0x2b2d26))
                    .child(label(notice.clone()).flex_1().min_w_0().text_size(px(11.)))
                    .child(
                        control(
                            "dismiss-connection-notice",
                            tr(cx, msg::chat::message_row_dismiss),
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.connection_notice = None;
                            cx.notify();
                        })),
                    ),
            );
        }
        if !self.uncertain_drafts.is_empty() {
            let rows = self
                .uncertain_drafts
                .iter()
                .enumerate()
                .map(|(index, draft)| {
                    row()
                        .gap_2()
                        .child(
                            label(if draft.text.is_empty() {
                                "Attachment message".into()
                            } else {
                                draft.text.chars().take(80).collect::<String>()
                            })
                            .flex_1()
                            .min_w_0()
                            .text_ellipsis(),
                        )
                        .child(
                            control(
                                ("restore-delivery", index),
                                tr(cx, msg::native::restore_draft),
                            )
                            .on_click(
                                cx.listener(move |this, _, _, cx| this.recover_draft(index, cx)),
                            ),
                        )
                        .child(
                            control(
                                ("dismiss-delivery", index),
                                tr(cx, msg::chat::message_row_dismiss),
                            )
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    if index < this.uncertain_drafts.len() {
                                        this.uncertain_drafts.remove(index);
                                    }
                                    cx.notify();
                                },
                            )),
                        )
                });
            view = view.child(
                column()
                    .id("uncertain-deliveries")
                    .max_h(px(160.))
                    .overflow_y_scroll()
                    .p_3()
                    .gap_2()
                    .rounded_lg()
                    .border_b_1()
                    .border_color(rgb(0x2b2d26))
                    .child(label(tr(
                        cx,
                        msg::native::recovered_drafts_review_chat_and_queue_before_restoring,
                    )))
                    .children(rows),
            );
        }
        view.into_any_element()
    }
}

#[cfg(test)]
mod hosted_session_tests {
    use super::*;
    use std::prelude::v1::test;

    #[gpui::test]
    fn hosted_session_replacement_revokes_actor_state_but_transport_retry_preserves_it(
        cx: &mut TestAppContext,
    ) {
        cx.update(initialize);
        let (requests, mut outgoing) = futures::channel::mpsc::unbounded();
        let (_events, events) = futures::channel::mpsc::unbounded();
        let window = cx.add_window(|window, cx| {
            WorkspaceView::new(Transport { requests, events }, window, cx)
        });
        window
            .update(cx, |view, _, cx| {
                view.connected = true;
                view.device_id = Some("old-cloud-controller".into());
                view.cloud.account = Some("previous-actor".into());
                view.cloud.target = Some(json!({"workspaceId":"protected-workspace"}));
                view.cloud.projects = vec![json!({"projectId":"private-project"})];
                view.attachments = vec![json!({"path":"private-attachment"})];
                view.input.update(cx, |input, cx| {
                    input.set_value("Private unsent draft".into(), cx)
                });
            })
            .unwrap();
        cx.run_until_parked();
        window
            .update(cx, |view, _, cx| {
                assert_eq!(view.input.read(cx).value(cx), "Private unsent draft");
                view.receive_connection(&json!({"connection":"retrying","interrupted":[]}), cx);
                assert_eq!(view.cloud.account.as_deref(), Some("previous-actor"));
                assert!(view.cloud.target.is_some());
                assert_eq!(view.attachments.len(), 1);
                assert_eq!(view.input.read(cx).value(cx), "Private unsent draft");
                view.receive_connection(
                    &json!({"connection":"retrying","controllerBusy":true}),
                    cx,
                );
                assert_eq!(view.error.as_deref(), Some(HOSTED_CONTROLLER_BUSY));
                assert_eq!(view.cloud.account.as_deref(), Some("previous-actor"));
                assert_eq!(view.input.read(cx).value(cx), "Private unsent draft");
                while outgoing.try_recv().is_ok() {}
                view.receive_connection(
                    &json!({"connection":"authRequired","hostedSessionRevoked":true}),
                    cx,
                );
                assert!(view.device_id.is_none());
                assert!(view.cloud.account.is_none());
                assert!(view.cloud.target.is_none());
                assert!(view.cloud.projects.is_empty());
                assert!(view.attachments.is_empty());
                assert!(view.send_draft.is_empty());
                assert!(view.uncertain_drafts.is_empty());
                while let Ok(frame) = outgoing.try_recv() {
                    if let Some(method) = frame["method"].as_str() {
                        assert!(
                            methods::retryable_read(method, &frame["params"]),
                            "session replacement must not replay a mutation"
                        );
                    }
                }
            })
            .unwrap();
        cx.run_until_parked();
        window
            .update(cx, |view, _, cx| {
                assert!(view.input.read(cx).value(cx).is_empty());
            })
            .unwrap();
    }
    #[gpui::test]
    fn hosted_controller_uuid_change_revokes_peer_tab_but_native_identity_change_preserves_draft(
        cx: &mut TestAppContext,
    ) {
        cx.update(initialize);
        for hosted in [false, true] {
            let (requests, mut outgoing) = futures::channel::mpsc::unbounded();
            let (_events, events) = futures::channel::mpsc::unbounded();
            let window = cx.add_window(|window, cx| {
                WorkspaceView::new(Transport { requests, events }, window, cx)
            });
            window
                .update(cx, |view, _, cx| {
                    view.device_id = Some("00000000-0000-4000-8000-000000000001".into());
                    view.cloud.account = Some("previous-actor".into());
                    view.cloud.target = Some(json!({"workspaceId":"protected-workspace"}));
                    view.input
                        .update(cx, |input, cx| input.set_value("Private draft".into(), cx));
                })
                .unwrap();
            cx.run_until_parked();
            window.update(cx,|view,_,cx|{
                assert_eq!(view.input.read(cx).value(cx),"Private draft");while outgoing.try_recv().is_ok(){}
                view.verify_engine(&json!({"deviceId":"00000000-0000-4000-8000-000000000002","capabilities":if hosted{vec!["cloud-controller-v1"]}else{vec![]}}),cx);
                if hosted {
                    assert!(view.cloud.account.is_none());assert!(view.cloud.target.is_none());
                    assert_eq!(view.device_id.as_deref(),Some("00000000-0000-4000-8000-000000000002"));
                    assert_eq!(view.status,"Connected to Zuse");
                    let mut revoked=false;
                    while let Ok(frame)=outgoing.try_recv(){
                        revoked |= frame["ui"]=="hostedSessionRevoked";
                        if let Some(method)=frame["method"].as_str(){
                            // Retry classification is deliberately more conservative
                            // than read-only startup metadata (e.g. WatchDevices).
                            let bootstrap = matches!(method,methods::WATCH_DEVICES|methods::WATCH_HARNESS_UPDATES|methods::LIST_PAIRINGS|methods::BROWSER_ACCESS_STATUS|methods::RELAY_ACCESS_STATUS|methods::GET_TITLE_SETTINGS|methods::GET_WORKTREE_SETTINGS|methods::PLUGINS);
                            assert!(methods::retryable_read(method,&frame["params"])||bootstrap,"unexpected startup RPC: {method}");
                        }
                    }
                    assert!(revoked,"peer cookie replacement must revoke IndexedDB authority too");
                }else{
                    assert_eq!(view.cloud.account.as_deref(),Some("previous-actor"));assert!(view.cloud.target.is_some());
                    assert_eq!(view.device_id.as_deref(),Some("00000000-0000-4000-8000-000000000001"));
                }
            }).unwrap();
            cx.run_until_parked();
            window
                .update(cx, |view, _, cx| {
                    assert_eq!(
                        view.input.read(cx).value(cx),
                        if hosted { "" } else { "Private draft" }
                    )
                })
                .unwrap();
        }
    }
}
