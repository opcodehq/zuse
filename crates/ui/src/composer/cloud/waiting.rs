//! A message that has to wait for its computer lives in the server's queue,
//! not in this app: it survives closing the app, sleep and Update computer,
//! and every device shows it as a queued message.
//!
//! ```text
//! send ─▶ queued on the server { waitingFor } ─▶ update commits / recreation ready ─▶ sent
//!            │
//!            └─ had files ─▶ copied to server storage, the queued row switched to them
//!                            (deleted from storage once the computer has them)
//! ```
//!
//! Above the composer the queue's reason shows as a status: "Updating
//! computer…" while it updates, or Update computer when only a recreation
//! can update it. Older servers refuse instead; [`super::runtime_update`]
//! keeps that path working.
use super::launch_upload;
use super::runtime_update::{NotSent, Phase, RESTART_NOTE, UPDATING};
use super::*;
use crate::composer::tray::{TrayPill, TrayTone, tray_text_action};
use zuse_protocol::cloud_runtime_updates::{
    RECREATING, RUNTIME_UPDATE_UNAVAILABLE, RUNTIME_UPDATING,
};

/// What the server's queue is waiting for, as a status above the composer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Waiting {
    /// The computer is updating its runtime in place.
    Update,
    /// Update computer is recreating it.
    Recreation,
    /// It can't update in place: Update computer is needed.
    UpdateComputer,
}

pub(super) fn waiting(code: &str) -> Option<Waiting> {
    match code {
        RUNTIME_UPDATING => Some(Waiting::Update),
        RECREATING => Some(Waiting::Recreation),
        RUNTIME_UPDATE_UNAVAILABLE => Some(Waiting::UpdateComputer),
        _ => None,
    }
}

/// The queued row's input for a waiting message, from the run it was sent as.
fn queued_input(request: &Value) -> Option<Value> {
    let run = &request["action"]["payload"]["request"];
    if request["action"]["type"] != "submit" || !run.is_object() {
        return None;
    }
    Some(json!({
        "text": run["prompt"],
        "runSelection": {
            "harness": run["harness"],
            "model": run["model"],
            "reasoning": run["reasoning"],
            "modelOptions": run.get("modelOptions").cloned().unwrap_or(json!({})),
            "sandbox": run["sandbox"],
        },
    }))
}

/// The queued row with its files held in server storage instead of on the
/// computer, or `None` when it can't be built.
pub(super) fn edit_to_server_files(
    request: &Value,
    accepted: &Value,
    assets: &[zuse_protocol::cloud_assets::Asset],
) -> Option<Value> {
    let mut input = queued_input(request)?;
    input["attachments"] = json!(assets);
    Some(json!({
        "commandId": uuid::Uuid::new_v4().to_string(),
        "sessionId": request["sessionId"],
        "action": {
            "type": "edit",
            "queueId": accepted["id"].as_str()?,
            "revision": accepted["revision"].as_i64()?,
            "input": input,
        },
    }))
}

impl Composer {
    /// The server's queue status for this chat, if it waits for its computer.
    pub(super) fn cloud_queue_waiting(&self, cx: &App) -> Option<Waiting> {
        self.cloud.native.as_ref()?;
        waiting(self.state.read(cx).selected_cloud_queue_error()?)
    }
    /// Queued messages wait for Update computer: it can't update in place,
    /// or an earlier Update computer from here didn't finish.
    pub(super) fn cloud_queue_needs_update_computer(&self, cx: &App) -> bool {
        match self.cloud_queue_waiting(cx) {
            Some(Waiting::UpdateComputer) => {
                !matches!(self.cloud.runtime.phase, Some(Phase::Recreating { .. }))
            }
            Some(Waiting::Recreation) => matches!(
                self.cloud.runtime.phase,
                Some(Phase::NotSent(NotSent::RecreationFailed(_)))
            ),
            _ => false,
        }
    }
    /// The status above the composer while the server's queue waits for the
    /// computer. Remove and edit stay on the queued rows themselves.
    pub(super) fn cloud_queue_waiting_row(
        &self,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Option<TrayPill> {
        let waiting = self.cloud_queue_waiting(cx)?;
        if let Some(row) = self.waiting_copy_row(theme, cx) {
            return Some(row);
        }
        let spinner = |cx: &mut Context<Self>| -> gpui::AnyElement {
            crate::loaders::mini_mono_spinner(
                "cloud-queue-waiting-spinner",
                2.,
                theme.text_muted,
                cx.entity_id(),
                cx,
            )
            .into_any_element()
        };
        let update_computer = |theme: &Theme, cx: &mut Context<Self>| {
            tray_text_action("update-cloud-computer", "Update computer", None, theme)
                .on_click(cx.listener(|this, _, _, cx| this.update_cloud_computer(cx)))
        };
        if self.cloud_queue_needs_update_computer(cx) {
            let subtitle = match (&self.cloud.runtime.phase, waiting) {
                (Some(Phase::NotSent(NotSent::RecreationFailed(error))), _) => {
                    format!("The update didn't finish: {error} Your message stays queued.")
                }
                _ => format!(
                    "This computer needs an update to take your message, which stays queued. {RESTART_NOTE}"
                ),
            };
            return Some(
                TrayPill::new("cloud-queue-waiting", "Waiting for an update")
                    .subtitle(subtitle)
                    .tone(TrayTone::Warning)
                    .glyph(crate::icons::ALERT_CIRCLE)
                    .action(update_computer(theme, cx)),
            );
        }
        let recreating = waiting == Waiting::Recreation
            || matches!(self.cloud.runtime.phase, Some(Phase::Recreating { .. }));
        Some(
            TrayPill::new("cloud-queue-waiting", UPDATING)
                .subtitle(if recreating {
                    RESTART_NOTE
                } else {
                    "Your queued message sends as soon as it's done."
                })
                .icon_element(spinner(cx)),
        )
    }
    /// A message queued to wait for its computer had files on the computer.
    /// Copy them to server storage and point its queued row at them, so a
    /// recreated computer still gets them. A copy that runs out of attempts
    /// stays above the composer with Retry and Remove (send it without the
    /// file); meanwhile the message keeps its files on the computer. If the
    /// message was sent first, it keeps what it has.
    pub(super) fn keep_waiting_files_on_server(
        &mut self,
        request: &Value,
        accepted: &Value,
        staged: Vec<StagedAttachment>,
        cx: &mut Context<Self>,
    ) {
        if staged.is_empty() || self.cloud.native.is_none() {
            return;
        }
        self.cloud.waiting_copy = Some(WaitingCopy {
            request: request.clone(),
            accepted: accepted.clone(),
            staged,
            progress: Default::default(),
            failed: None,
            task: None,
        });
        self.copy_waiting_files(cx);
    }
    /// (Re)start the copy, skipping files that already committed.
    fn copy_waiting_files(&mut self, cx: &mut Context<Self>) {
        let Some(target) = self.cloud.native.as_ref().map(|task| task.target.clone()) else {
            return;
        };
        let Some(copy) = self.cloud.waiting_copy.as_mut() else {
            return;
        };
        let progress = launch_upload::Progress::resume(&copy.staged, Some(&copy.progress));
        copy.progress = progress.clone();
        copy.failed = None;
        let (request, accepted, staged) = (
            copy.request.clone(),
            copy.accepted.clone(),
            copy.staged.clone(),
        );
        copy.task = Some(cx.spawn(async move |this, cx| {
            let executor = cx.background_executor().clone();
            let result = {
                let call = launch_upload::asset_call(
                    &target.controller,
                    &target.workspace,
                    &target.subject,
                    target.organization.as_deref(),
                );
                launch_upload::upload_files(&call, &executor, &staged, &progress).await
            };
            if let Err(failure) = result {
                tracing::warn!(error = %failure.error, "a waiting message's file didn't reach server storage");
                let _ = this.update(cx, |this, cx| {
                    if let Some(copy) = this.cloud.waiting_copy.as_mut() {
                        copy.task = None;
                        copy.failed = Some((failure.file, failure.name));
                    }
                    cx.notify();
                });
                return;
            }
            let assets: Vec<zuse_protocol::cloud_assets::Asset> = progress
                .assets(&staged)
                .into_iter()
                .filter_map(|asset| serde_json::from_value(asset).ok())
                .collect();
            let edit = edit_to_server_files(&request, &accepted, &assets);
            if let Some(edit) = edit
                && let Err(error) = target
                    .controller
                    .client()
                    .call_scoped(
                        methods::CLOUD_TASK_COMMAND,
                        json!({"workspaceId":target.workspace,"expectedAccountId":target.subject,
                            "expectedOrganizationId":target.organization,"request":edit}),
                    )
                    .await
            {
                // Already sent with its files on the computer, or changed by
                // someone: either way it keeps what it has.
                tracing::info!(%error, "A waiting message kept its files on the computer");
            }
            let _ = this.update(cx, |this, cx| {
                this.cloud.waiting_copy = None;
                cx.notify();
            });
        }));
        cx.notify();
    }
    /// The file whose copy ran out of attempts, while its message still waits.
    pub(super) fn waiting_copy_failed(&self, cx: &App) -> Option<&str> {
        self.cloud_queue_waiting(cx)?;
        let copy = self.cloud.waiting_copy.as_ref()?;
        copy.failed.as_ref().map(|(_, name)| name.as_str())
    }
    /// Retry the copy of the waiting message's files.
    pub(super) fn retry_waiting_copy(&mut self, cx: &mut Context<Self>) {
        if self
            .cloud
            .waiting_copy
            .as_ref()
            .is_some_and(|copy| copy.failed.is_some())
        {
            self.copy_waiting_files(cx);
        }
    }
    /// Send the waiting message without the file that wouldn't copy.
    pub(super) fn remove_waiting_copy_file(&mut self, cx: &mut Context<Self>) {
        let Some(copy) = self.cloud.waiting_copy.as_mut() else {
            return;
        };
        let Some((failed, _)) = copy.failed.clone() else {
            return;
        };
        copy.staged.retain(|file| file.id != failed);
        self.copy_waiting_files(cx);
    }
    fn waiting_copy_row(&self, theme: &Theme, cx: &mut Context<Self>) -> Option<TrayPill> {
        let name = self.waiting_copy_failed(cx)?;
        Some(
            TrayPill::new(
                "cloud-waiting-copy",
                format!("Couldn't save {name} for your queued message"),
            )
            .subtitle("It stays on the computer for now. Retry saves it; Remove sends the message without it.")
            .tone(TrayTone::Danger)
            .glyph(crate::icons::ALERT_CIRCLE)
            .action(
                tray_text_action("retry-cloud-waiting-copy", "Retry", None, theme)
                    .debug_selector(|| "retry-cloud-waiting-copy".into())
                    .on_click(cx.listener(|this, _, _, cx| this.retry_waiting_copy(cx))),
            )
            .action(
                tray_text_action("remove-cloud-waiting-copy", "Remove", None, theme)
                    .debug_selector(|| "remove-cloud-waiting-copy".into())
                    .on_click(cx.listener(|this, _, _, cx| this.remove_waiting_copy_file(cx))),
            ),
        )
    }
}

/// A waiting message's files being copied to server storage.
pub(in crate::composer) struct WaitingCopy {
    request: Value,
    accepted: Value,
    staged: Vec<StagedAttachment>,
    progress: launch_upload::Progress,
    /// The file (id, name) that ran out of attempts.
    failed: Option<(String, String)>,
    task: Option<Task<()>>,
}

#[cfg(test)]
mod tests {
    use super::super::runtime_update::tests::{
        composer_with_requests, echoed, next, reply, runtime, settle,
    };
    use super::*;

    /// The server queues a message that has to wait for its computer: the app
    /// holds nothing, the queued row replaces the bubble, and the status comes
    /// from the server's queue, so it survives this composer going away.
    #[gpui::test]
    fn a_message_waiting_for_its_computer_lives_in_the_servers_queue(
        cx: &mut gpui::TestAppContext,
    ) {
        let runtime = runtime();
        let _entered = runtime.enter();
        let (composer, state, mut requests, replies) = composer_with_requests(cx);
        composer.update(cx, |c, cx| {
            c.input
                .update(cx, |input, cx| input.set_text("Fix the build", cx));
            assert!(c.send_cloud_task("Fix the build".into(), false, cx));
        });
        settle(cx, &runtime);
        let request = next(&mut requests).expect("the message is sent");
        assert_eq!(request["method"], methods::CLOUD_TASK_COMMAND);
        assert_eq!(
            request["params"][zuse_protocol::cloud_runtime_updates::QUEUE_WHILE_WAITING],
            true,
            "it asks the server to queue it rather than refuse it"
        );
        let message = request["params"]["request"]["action"]["messageId"]
            .as_str()
            .unwrap()
            .to_owned();
        reply(
            &runtime,
            &replies,
            json!({"id":request["id"],"ok":{"accountId":"owner","organizationId":null,
                "data":{"commandId":request["params"]["request"]["commandId"],"durablyAdmitted":true,
                    "id":message,"messageId":message,"revision":1,"state":"queued",
                    "waitingFor":RUNTIME_UPDATING}}}),
        );
        settle(cx, &runtime);
        composer.update(cx, |c, cx| {
            assert!(c.cloud.pending_native.is_none());
            assert!(c.cloud.admitted_native.is_none(), "not a running turn");
            assert!(c.cloud.runtime.phase.is_none(), "nothing held in the app");
            assert!(c.failure.is_none());
            assert_eq!(c.input.read(cx).text(), "");
            // A new message goes to the server too, not into an app hold.
            assert!(!c.cloud_runtime_update_holds());
        });
        assert_eq!(
            echoed(&state, cx).len(),
            1,
            "its bubble stays until its row arrives"
        );
        // The server's queue arrives with its reason: the row replaces the
        // bubble and the status reads from the queue.
        state.update(cx, |state, _| {
            state.cloud_queue_error = Some(("session".into(), RUNTIME_UPDATING.into()));
            state.apply_queue(vec![zuse_doc::QueuedMessage {
                id: message.clone(),
                text: "Fix the build".into(),
                attachments: vec![],
                hold_for_turn_end: true,
                issued_by: "cloud-authority".into(),
                issued_at: 0,
                edited_at: None,
                delivery_gate: None,
            }]);
        });
        assert!(echoed(&state, cx).is_empty());
        composer.read_with(cx, |c, cx| {
            assert_eq!(c.cloud_queue_waiting(cx), Some(Waiting::Update));
            assert!(!c.cloud_queue_needs_update_computer(cx));
        });
        // It can't update in place: Update computer is offered.
        state.update(cx, |state, _| {
            state.cloud_queue_error = Some(("session".into(), RUNTIME_UPDATE_UNAVAILABLE.into()));
        });
        composer.read_with(cx, |c, cx| {
            assert!(c.cloud_queue_needs_update_computer(cx));
        });
        // Sent: the queue empties and the status goes with it.
        state.update(cx, |state, _| {
            state.cloud_queue_error = None;
            state.apply_queue(vec![]);
        });
        composer.read_with(cx, |c, cx| assert_eq!(c.cloud_queue_waiting(cx), None));
    }

    #[test]
    fn a_waiting_message_switches_to_server_files_with_its_settings_intact() {
        let request = json!({"commandId":"c","sessionId":"s","action":{"type":"submit","messageId":"m",
            "payload":{"kind":"run","messageId":"m","request":{"prompt":"look","harness":"codex","model":"fixture",
            "reasoning":"high","modelOptions":{"speed":"fast"},"cwd":".","sandbox":"danger-full-access","autoApprove":true,
            "attachments":["/home/zuse/.zuse-data/uploads/a-shot.png"]}}}});
        let asset = zuse_protocol::cloud_assets::Asset {
            asset_id: "asset_a".into(),
            mime_type: "image/png".into(),
            original_name: "shot.png".into(),
            size_bytes: 3,
            sha256: "digest".into(),
        };
        let edit = edit_to_server_files(
            &request,
            &json!({"id":"m","revision":1,"state":"queued"}),
            std::slice::from_ref(&asset),
        )
        .unwrap();
        let parsed: zuse_protocol::cloud_task_commands::Request =
            serde_json::from_value(edit.clone()).unwrap();
        let zuse_protocol::cloud_task_commands::Action::Edit {
            queue_id,
            revision,
            input,
            lease_id,
        } = parsed.action
        else {
            panic!("an edit of the queued row");
        };
        assert_eq!((queue_id.as_str(), revision, lease_id), ("m", 1, None));
        assert_eq!(parsed.session_id, "s");
        assert_ne!(parsed.command_id, "c");
        assert_eq!(input.text, "look");
        assert!(input.attachments == [asset]);
        assert!(input.computer_files.is_empty());
        let selection = input.run_selection.unwrap();
        assert_eq!(selection.model, "fixture");
        assert_eq!(
            selection.sandbox,
            zuse_protocol::SandboxLevel::DangerFullAccess
        );
        assert_eq!(selection.model_options["speed"], "fast");
    }

    /// Server storage as it really behaves: every piece but the last is
    /// exactly `CHUNK_BYTES`, and a commit needs every piece. Fails every
    /// asset call while `failing`.
    #[derive(Default)]
    struct Storage {
        calls: std::sync::Mutex<Vec<(String, Value)>>,
        pieces: std::sync::Mutex<std::collections::BTreeMap<(String, u64), usize>>,
        failing: std::sync::atomic::AtomicBool,
    }
    impl Storage {
        fn edits(&self) -> Vec<Value> {
            self.calls
                .lock()
                .unwrap()
                .iter()
                .filter(|(method, _)| method == methods::CLOUD_TASK_COMMAND)
                .map(|(_, params)| params["request"].clone())
                .collect()
        }
    }
    #[async_trait::async_trait]
    impl zuse_rpc::RpcService for Storage {
        async fn handle(
            &self,
            method: &str,
            params: Value,
        ) -> Result<zuse_rpc::RpcReply, zuse_rpc::RpcError> {
            use base64::Engine as _;
            use sha2::Digest as _;
            use zuse_protocol::cloud_assets::CHUNK_BYTES;
            self.calls
                .lock()
                .unwrap()
                .push((method.into(), params.clone()));
            if method != methods::CLOUD_TASK_ASSET {
                return zuse_rpc::RpcReply::value(&json!({"data":{"durablyAdmitted":true}}));
            }
            if self.failing.load(std::sync::atomic::Ordering::SeqCst) {
                return Err(zuse_rpc::RpcError::Failed(
                    "Cloud service is unavailable.".into(),
                ));
            }
            let call = &params["params"];
            let upload = call["uploadId"].as_str().unwrap().to_owned();
            if params["method"] == methods::UPLOAD_CHUNK {
                let len = base64::engine::general_purpose::STANDARD
                    .decode(call["data"].as_str().unwrap())
                    .unwrap()
                    .len();
                if len > CHUNK_BYTES {
                    return Err(zuse_rpc::RpcError::Failed("invalid_launch_asset".into()));
                }
                self.pieces
                    .lock()
                    .unwrap()
                    .insert((upload, call["seq"].as_u64().unwrap()), len);
                return zuse_rpc::RpcReply::value(&json!({"data":{}}));
            }
            let pieces: Vec<usize> = self
                .pieces
                .lock()
                .unwrap()
                .iter()
                .filter(|((id, _), _)| *id == upload)
                .map(|(_, len)| *len)
                .collect();
            let size: usize = pieces.iter().sum();
            let whole = pieces[..pieces.len() - 1]
                .iter()
                .all(|len| *len == CHUNK_BYTES);
            if !whole || pieces.len() != size.div_ceil(CHUNK_BYTES) {
                return Err(zuse_rpc::RpcError::Failed("launch_asset_incomplete".into()));
            }
            let bytes = vec![7u8; size];
            zuse_rpc::RpcReply::value(&json!({"data":{"asset":{
                "assetId":format!("asset_{upload}"),"mimeType":"image/png",
                "originalName":call["fileName"],"sizeBytes":size,
                "sha256":base64::engine::general_purpose::URL_SAFE_NO_PAD
                    .encode(sha2::Sha256::digest(&bytes))}}}))
        }
    }

    /// A chat whose message waits for its computer (it is updating), backed
    /// by `storage`.
    fn waiting_chat(
        cx: &mut gpui::TestAppContext,
        storage: std::sync::Arc<Storage>,
    ) -> Entity<Composer> {
        let engine = crate::state::EngineHandle::from_test_client(zuse_rpc::memory_client(storage));
        let state = cx.new(|_| {
            let mut state = crate::state::AppState::new();
            state.apply_cloud_account(json!({"user":{"id":"owner"},"orgId":null}));
            state.selected_chat = Some("session".into());
            state.cloud_queue_error = Some(("session".into(), RUNTIME_UPDATING.into()));
            state.apply_queue(vec![zuse_doc::QueuedMessage {
                id: "m".into(),
                text: "look".into(),
                attachments: vec![],
                hold_for_turn_end: true,
                issued_by: "cloud-authority".into(),
                issued_at: 0,
                edited_at: None,
                delivery_gate: None,
            }]);
            state
        });
        let composer = cx.new(|cx| Composer::new(state, cx));
        composer.update(cx, |c, cx| {
            let target = crate::state::CloudTarget {
                controller: engine,
                subject: "owner".into(),
                organization: None,
                workspace: "task".into(),
                initial_session: "session".into(),
            };
            c.set_cloud_task(
                target,
                json!({"agent":"codex","model":"gpt-6.1-sol","runtimeMode":"full-access"}),
                cx,
            );
        });
        composer
    }

    fn waiting_run() -> Value {
        json!({"commandId":"c","sessionId":"session","action":{"type":"submit","messageId":"m",
            "payload":{"kind":"run","messageId":"m","request":{"prompt":"look","harness":"codex",
            "model":"gpt-6.1-sol","cwd":".","sandbox":"workspace-write","attachments":[]}}}})
    }

    fn drive(
        cx: &mut gpui::TestAppContext,
        runtime: &tokio::runtime::Runtime,
        mut done: impl FnMut(&mut gpui::TestAppContext) -> bool,
    ) {
        for _ in 0..400 {
            cx.run_until_parked();
            if done(cx) {
                return;
            }
            runtime.block_on(tokio::time::sleep(std::time::Duration::from_millis(2)));
            cx.executor()
                .advance_clock(std::time::Duration::from_secs(1));
        }
        panic!("the copy did not get there");
    }

    /// A file over 1 MB reaches server storage in the pieces the server
    /// commits, and the queued row switches to it.
    #[gpui::test]
    fn a_large_waiting_file_reaches_server_storage(cx: &mut gpui::TestAppContext) {
        let runtime = runtime();
        let _entered = runtime.enter();
        let storage = std::sync::Arc::new(Storage::default());
        let composer = waiting_chat(cx, storage.clone());
        let shot = StagedAttachment::from_file("shot.png", vec![7u8; 1_300_000]);
        composer.update(cx, |c, cx| {
            c.keep_waiting_files_on_server(
                &waiting_run(),
                &json!({"id":"m","revision":1}),
                vec![shot.clone()],
                cx,
            )
        });
        drive(cx, &runtime, |_| !storage.edits().is_empty());
        let edit = storage.edits()[0].clone();
        assert_eq!(edit["action"]["type"], "edit");
        assert_eq!(
            edit["action"]["input"]["attachments"][0]["sizeBytes"],
            1_300_000
        );
        assert_eq!(
            edit["action"]["input"]["attachments"][0]["assetId"],
            format!("asset_{}", shot.id)
        );
        let pieces: Vec<usize> = storage.pieces.lock().unwrap().values().copied().collect();
        assert_eq!(pieces.len(), 3);
        composer.read_with(cx, |c, _| assert!(c.cloud.waiting_copy.is_none()));
    }

    /// A copy that keeps failing shows above the composer with Retry and
    /// Remove instead of only a log line. Retry saves the file; Remove sends
    /// the message without it.
    #[gpui::test]
    fn a_failed_waiting_copy_offers_retry_and_remove(cx: &mut gpui::TestAppContext) {
        let runtime = runtime();
        let _entered = runtime.enter();
        let storage = std::sync::Arc::new(Storage::default());
        storage
            .failing
            .store(true, std::sync::atomic::Ordering::SeqCst);
        let composer = waiting_chat(cx, storage.clone());
        let shot = StagedAttachment::from_file("shot.png", vec![7u8; 1_100_000]);
        let start = |composer: &Entity<Composer>, cx: &mut gpui::TestAppContext| {
            composer.update(cx, |c, cx| {
                c.keep_waiting_files_on_server(
                    &waiting_run(),
                    &json!({"id":"m","revision":1}),
                    vec![shot.clone()],
                    cx,
                )
            })
        };
        start(&composer, cx);
        drive(cx, &runtime, |cx| {
            composer.read_with(cx, |c, cx| c.waiting_copy_failed(cx).is_some())
        });
        composer.read_with(cx, |c, cx| {
            assert_eq!(c.waiting_copy_failed(cx), Some("shot.png"));
        });
        assert!(
            storage.edits().is_empty(),
            "the row keeps its computer file"
        );
        // Retry, with storage back: the file is saved and the row switched.
        storage
            .failing
            .store(false, std::sync::atomic::Ordering::SeqCst);
        composer.update(cx, |c, cx| c.retry_waiting_copy(cx));
        drive(cx, &runtime, |_| !storage.edits().is_empty());
        assert_eq!(
            storage.edits()[0]["action"]["input"]["attachments"][0]["sizeBytes"],
            1_100_000
        );
        // Remove: the message goes without the file.
        storage
            .failing
            .store(true, std::sync::atomic::Ordering::SeqCst);
        start(&composer, cx);
        drive(cx, &runtime, |cx| {
            composer.read_with(cx, |c, cx| c.waiting_copy_failed(cx).is_some())
        });
        composer.update(cx, |c, cx| c.remove_waiting_copy_file(cx));
        drive(cx, &runtime, |_| storage.edits().len() == 2);
        assert_eq!(
            storage.edits()[1]["action"]["input"]["attachments"],
            json!([])
        );
        composer.read_with(cx, |c, cx| {
            assert!(c.waiting_copy_failed(cx).is_none());
            assert!(c.cloud.waiting_copy.is_none());
        });
    }
}
