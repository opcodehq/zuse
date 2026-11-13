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
use super::runtime_update::{NotSent, Phase, RESTART_NOTE, UPDATING};
use super::*;
use crate::composer::tray::{TrayPill, TrayTone, tray_text_action};
use futures::FutureExt as _;
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
    /// recreated computer still gets them. Best effort: if this fails, or the
    /// message was sent first, it keeps its files on the computer.
    pub(super) fn keep_waiting_files_on_server(
        &mut self,
        request: &Value,
        accepted: &Value,
        staged: Vec<StagedAttachment>,
        cx: &mut Context<Self>,
    ) {
        if staged.is_empty() {
            return;
        }
        let Some(task) = self.cloud.native.as_ref() else {
            return;
        };
        let target = task.target.clone();
        let request = request.clone();
        let accepted = accepted.clone();
        cx.spawn(async move |_, cx| {
            let executor = cx.background_executor().clone();
            let call = |method: &'static str, params: Value, _timeout: std::time::Duration| {
                let target = target.clone();
                async move {
                    target
                        .controller
                        .client()
                        .call_scoped(
                            methods::CLOUD_TASK_ASSET,
                            json!({"workspaceId":target.workspace,"expectedAccountId":target.subject,
                                "expectedOrganizationId":target.organization,"method":method,"params":params}),
                        )
                        .await
                        .map(|reply| reply["data"].clone())
                        .map_err(|error| error.to_string())
                }
                .boxed_local()
            };
            let mut assets = Vec::with_capacity(staged.len());
            for file in &staged {
                let asset = attachments::upload_with(&call, &executor, &file.id, file, None)
                    .await
                    .and_then(|path| {
                        zuse_engine::cloud::task_commands::parse_asset_ref(&path)
                            .map_err(|error| error.to_string())
                    });
                match asset {
                    Ok(asset) => assets.push(asset),
                    Err(error) => {
                        tracing::warn!(%error, "A waiting message keeps its files on the computer");
                        return;
                    }
                }
            }
            let Some(edit) = edit_to_server_files(&request, &accepted, &assets) else {
                return;
            };
            if let Err(error) = target
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
        })
        .detach();
    }
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
    fn only_waiting_for_the_computer_is_a_status() {
        assert_eq!(waiting(RUNTIME_UPDATING), Some(Waiting::Update));
        assert_eq!(waiting(RECREATING), Some(Waiting::Recreation));
        assert_eq!(
            waiting(RUNTIME_UPDATE_UNAVAILABLE),
            Some(Waiting::UpdateComputer)
        );
        assert_eq!(waiting("cloud_task_queue_author_changed"), None);
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
}
