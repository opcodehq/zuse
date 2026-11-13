//! A message whose files went missing from its computer before it ran (a
//! restore or recreate replaced the computer's files) is sent again on its
//! own, once, with its files uploaded again:
//!
//! ```text
//! send ─▶ admitted ─▶ kept (files held here)
//!                       ├─ computer takes it (starting, running, done, in the transcript) ─▶ let go
//!                       ├─ refused "file missing" ─▶ upload files again ─▶ send again (once) ─▶ kept
//!                       └─ refused again, or for another reason ─▶ let go; the chat explains
//! ```
//!
//! Meanwhile the chat shows the message waiting for the agent, never refused.
//! Nothing here survives a restart: after one, the chat explains and the
//! files are attached again by hand.
use super::runtime_update::{Held, Phase, message_id};
use super::*;

/// How many admitted messages keep their files at once; older ones let go.
const KEPT: usize = 8;

/// Refusal codes meaning a message's files weren't on its computer.
pub(super) fn files_missing(code: &str) -> bool {
    matches!(
        code,
        "cloud_command_upload_missing" | "cloud_command_upload_outside_uploads"
    )
}

/// An admitted message with files, until its computer takes it.
struct Kept {
    held: Held,
    /// Already sent again once: another refusal is final.
    resent: bool,
}

#[derive(Default)]
pub(in crate::composer) struct State {
    kept: Vec<Kept>,
    /// Messages already sent again, newest last (bounded).
    resent: std::collections::VecDeque<String>,
}

impl Composer {
    /// Keep an admitted message's files until its computer takes it.
    pub(super) fn keep_cloud_files(
        &mut self,
        request: &Value,
        text: Option<String>,
        staged: Vec<StagedAttachment>,
    ) {
        if staged.is_empty() {
            return;
        }
        let Some(id) = message_id(request) else {
            return;
        };
        let state = &mut self.cloud.reupload;
        let resent = state.resent.iter().any(|resent| resent == id);
        state
            .kept
            .retain(|kept| message_id(&kept.held.request) != Some(id));
        state.kept.push(Kept {
            held: Held {
                request: request.clone(),
                text,
                staged,
            },
            resent,
        });
        if state.kept.len() > KEPT {
            state.kept.remove(0);
        }
    }
    /// Follow kept messages through their command rows: let one go once its
    /// computer takes it, send it again if its files went missing.
    pub(super) fn observe_cloud_reupload(&mut self, admissions: &Value, cx: &mut Context<Self>) {
        let Some(rows) = admissions.as_array() else {
            return;
        };
        let row = |id: &str| rows.iter().find(|row| row["messageId"] == id);
        let mut again = Vec::new();
        self.cloud.reupload.kept.retain(|kept| {
            let Some(row) = message_id(&kept.held.request).and_then(row) else {
                return true;
            };
            match row["executionState"].as_str() {
                None | Some("queued" | "admitted" | "accepted") => true,
                Some("failed")
                    if !kept.resent && row["errorCode"].as_str().is_some_and(files_missing) =>
                {
                    again.push(kept.held.clone());
                    false
                }
                _ => false,
            }
        });
        for held in again {
            // Its admission comes later and finds it marked as resent.
            if let Some(id) = self.send_cloud_message_again(held, cx) {
                let resent = &mut self.cloud.reupload.resent;
                resent.push_back(id);
                if resent.len() > KEPT * 2 {
                    resent.pop_front();
                }
            }
        }
    }
    /// A kept message in the transcript ran: let it go.
    pub(super) fn observe_cloud_reupload_transcript(
        &mut self,
        messages: &[zuse_model::SessionMessageEntry],
    ) {
        self.cloud.reupload.kept.retain(|kept| {
            message_id(&kept.held.request)
                .is_none_or(|id| !messages.iter().any(|entry| entry.id == id))
        });
    }
    /// A message is on its way again because its files went missing, until
    /// its computer takes it: the chat shows it waiting for the agent rather
    /// than refused.
    pub(super) fn cloud_reupload_in_flight(&self) -> bool {
        let cloud = &self.cloud;
        // A message shown as not sent, or back in the draft with an error,
        // is no longer on its way.
        let waiting = !matches!(cloud.runtime.phase, Some(Phase::NotSent(_)));
        cloud.reupload.kept.iter().any(|kept| kept.resent)
            || cloud.reupload.resent.iter().any(|id| {
                (waiting && cloud.runtime.holds(id))
                    || (cloud.task.is_some()
                        && cloud.pending_native.as_ref().and_then(message_id) == Some(id.as_str()))
            })
    }
}

#[cfg(test)]
mod tests {
    use super::super::runtime_update::tests::{
        admit, composer_with_requests, echoed, next, reply, runtime, settle,
    };
    use super::*;

    /// Answer the computer's upload calls until the message itself goes out;
    /// returns it and how many files were uploaded.
    fn deliver(
        cx: &mut gpui::TestAppContext,
        runtime: &tokio::runtime::Runtime,
        requests: &mut tokio::sync::mpsc::Receiver<String>,
        replies: &tokio::sync::mpsc::Sender<String>,
        path: &str,
    ) -> (Value, usize) {
        let mut commits = 0;
        for _ in 0..16 {
            settle(cx, runtime);
            let Some(request) = next(requests) else {
                continue;
            };
            match request["method"].as_str() {
                Some(methods::UPLOAD_CHUNK) => {
                    assert_eq!(
                        request["params"]["computer"], true,
                        "straight to the computer"
                    );
                    reply(
                        runtime,
                        replies,
                        json!({"id":request["id"],"ok":{"ok":true}}),
                    );
                }
                Some(methods::UPLOAD_COMMIT) => {
                    commits += 1;
                    reply(
                        runtime,
                        replies,
                        json!({"id":request["id"],"ok":{"path":path}}),
                    );
                }
                Some(methods::CLOUD_TASK_COMMAND) => return (request, commits),
                other => panic!("unexpected request {other:?}"),
            }
        }
        panic!("the message never went out");
    }
    fn row(request: &Value, state: &str, code: Option<&str>) -> Value {
        let request = &request["params"]["request"];
        json!([{"commandId":request["commandId"],"messageId":request["action"]["messageId"],
            "text":"Look at this","executionState":state,"errorCode":code,"createdAt":2}])
    }
    /// A cloud chat whose computer is connected sends one message with a file.
    fn connected(
        cx: &mut gpui::TestAppContext,
    ) -> (
        Entity<Composer>,
        Entity<AppState>,
        tokio::sync::mpsc::Receiver<String>,
        tokio::sync::mpsc::Sender<String>,
    ) {
        let (composer, state, requests, replies) = composer_with_requests(cx);
        composer.update(cx, |c, cx| {
            let engine = c.cloud.native.as_ref().unwrap().target.controller.clone();
            c.state.update(cx, |state, _| {
                state.set_test_engine(engine);
                state.connection = crate::state::ConnectionStatus::Ready;
            });
            c.attachments.insert(
                c.current_key.clone(),
                vec![StagedAttachment::from_file(
                    "shot.png",
                    b"png bytes".to_vec(),
                )],
            );
            c.input
                .update(cx, |input, cx| input.set_text("Look at this", cx));
            assert!(c.send_cloud_task("Look at this".into(), false, cx));
        });
        (composer, state, requests, replies)
    }

    #[gpui::test]
    fn a_message_whose_file_went_missing_uploads_it_again_and_resends_once(
        cx: &mut gpui::TestAppContext,
    ) {
        let runtime = runtime();
        let _entered = runtime.enter();
        let (composer, state, mut requests, replies) = connected(cx);
        let (first, uploads) = deliver(cx, &runtime, &mut requests, &replies, "/u/a-shot.png");
        assert_eq!(uploads, 1);
        admit(&runtime, &replies, &first);
        settle(cx, &runtime);
        composer.update(cx, |c, _| {
            assert!(c.cloud.native_staged.is_empty());
            assert_eq!(
                c.cloud.reupload.kept.len(),
                1,
                "its file is kept until the computer takes it"
            );
        });

        // The computer's files were replaced: it refuses the message.
        composer.update(cx, |c, cx| {
            c.apply_cloud_admission_activity(
                &row(&first, "failed", Some("cloud_command_upload_missing")),
                cx,
            );
            assert_eq!(
                c.cloud_activity(),
                Some(("Waiting for agent…", true)),
                "never shown as refused while it recovers"
            );
            assert!(c.failure.is_none());
        });
        let (again, uploads) = deliver(cx, &runtime, &mut requests, &replies, "/u/b-shot.png");
        assert_eq!(uploads, 1, "the file goes to the computer again");
        let request = &again["params"]["request"];
        let original = &first["params"]["request"];
        assert_ne!(request["commandId"], original["commandId"]);
        assert_ne!(
            request["action"]["messageId"],
            original["action"]["messageId"]
        );
        assert_eq!(
            request["action"]["payload"]["request"]["prompt"],
            "Look at this"
        );
        assert_eq!(
            request["action"]["payload"]["request"]["attachments"],
            json!(["/u/b-shot.png"])
        );
        assert_eq!(
            echoed(&state, cx),
            vec![(
                request["action"]["messageId"].as_str().unwrap().to_owned(),
                "Look at this".to_owned()
            )],
            "the same message in the chat, under its new identity"
        );
        admit(&runtime, &replies, &again);
        settle(cx, &runtime);
        composer.update(cx, |c, cx| {
            assert_eq!(c.cloud_activity(), Some(("Waiting for agent…", true)));
            // Refused again: once is enough; the chat explains instead.
            c.apply_cloud_admission_activity(
                &row(
                    &again,
                    "failed",
                    Some("cloud_command_upload_outside_uploads"),
                ),
                cx,
            );
            assert!(c.cloud.reupload.kept.is_empty());
            assert!(!c.cloud_reupload_in_flight());
        });
        settle(cx, &runtime);
        assert!(next(&mut requests).is_none(), "never a loop");
    }

    #[gpui::test]
    fn a_message_its_computer_took_lets_its_file_go(cx: &mut gpui::TestAppContext) {
        let runtime = runtime();
        let _entered = runtime.enter();
        let (composer, _state, mut requests, replies) = connected(cx);
        let (first, _) = deliver(cx, &runtime, &mut requests, &replies, "/u/a.png");
        admit(&runtime, &replies, &first);
        settle(cx, &runtime);
        composer.update(cx, |c, cx| {
            c.apply_cloud_admission_activity(&row(&first, "queued", None), cx);
            assert_eq!(c.cloud.reupload.kept.len(), 1, "queued: the file stays");
            c.apply_cloud_admission_activity(&row(&first, "running", None), cx);
            assert!(
                c.cloud.reupload.kept.is_empty(),
                "running: the computer has it"
            );
        });
        settle(cx, &runtime);
        assert!(next(&mut requests).is_none());
    }

    #[gpui::test]
    fn a_refusal_for_another_reason_is_not_resent(cx: &mut gpui::TestAppContext) {
        let runtime = runtime();
        let _entered = runtime.enter();
        let (composer, _state, mut requests, replies) = connected(cx);
        let (first, _) = deliver(cx, &runtime, &mut requests, &replies, "/u/a.png");
        admit(&runtime, &replies, &first);
        settle(cx, &runtime);
        composer.update(cx, |c, cx| {
            c.apply_cloud_admission_activity(
                &row(&first, "failed", Some("cloud_command_model_changed")),
                cx,
            );
            assert!(c.cloud.reupload.kept.is_empty());
            assert!(!c.cloud_reupload_in_flight());
        });
        settle(cx, &runtime);
        assert!(next(&mut requests).is_none());
    }
}
