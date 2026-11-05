use super::*;
use std::prelude::v1::test;

#[gpui::test]
fn api_admissions_do_not_enter_guest_delta_indices_or_duplicate_canonical_messages(
    cx: &mut gpui::TestAppContext,
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
    window.update(cx,|view,_,_| {
        view.selected=Some(json!({"id":"session","cwd":"."}));
        let rows=vec![json!({"messageId":"a","text":"already canonical","attachments":[],"createdAt":1}),json!({"messageId":"b","text":"waiting","attachments":[],"createdAt":2})];
        view.messages=serde_json::from_value(json!([{"id":"a","role":"user","parts":[{"id":"canonical","kind":"text","text":"canonical wins"}],"createdAt":1,"deviceId":"runtime"}])).unwrap();
        view.merge_cloud_admissions(&rows);
        assert_eq!(view.messages.len(),2);
        assert!(matches!(&view.messages[0].parts[0],zuse_model::MessagePart::Text{text,..} if text=="canonical wins"));
        view.merge_cloud_admissions(&rows); assert_eq!(view.messages.len(),2);
        view.clear_cloud_admission_overlay(); assert_eq!(view.messages.len(),1,"guest index space contains only canonical rows");
        let canonical:SessionMessageEntry=serde_json::from_value(json!({"id":"b","role":"user","parts":[{"id":"canonical-b","kind":"text","text":"guest b"}],"createdAt":2,"deviceId":"runtime"})).unwrap();
        view.messages.push(canonical);
        view.restore_cloud_admission_overlay();assert_eq!(view.messages.len(),2);
        view.clear_cloud_admission_overlay();assert_eq!(view.messages.len(),2,"the guest row supersedes the provisional row");
        assert!(matches!(&view.messages[1].parts[0],zuse_model::MessagePart::Text{text,..} if text=="guest b"));
    }).unwrap();
}

#[gpui::test]
fn unknown_recovery_allows_only_fresh_work_and_never_retries_the_original(
    cx: &mut gpui::TestAppContext,
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
    window.update(cx,|view,_,cx|{
        view.selected=Some(json!({"id":"session","deviceId":"cloud:task","cwd":"."}));view.queue_paused=true;
        view.cloud.guest=true;view.cloud.guest_status=json!({"commandDeliveryMode":"server-admitted-v2","commandAdmissionVersion":2,"commandApiAuthorized":true,"taskCommandCanEdit":true});
        let rows=vec![json!({"sessionId":"session","commandId":"old-interrupted-control","messageId":null,"executionState":"outcome-unknown","unknownReviewed":false,"errorCode":"delivery-outcome-unknown"})];
        view.merge_cloud_admissions(&rows);assert!(view.messages.is_empty(),"recovery metadata must not fabricate a user message");assert_eq!(view.cloud_unreviewed_command().as_deref(),Some("old-interrupted-control"));
        view.review_unknown_command(cx);let (id,_,_)=view.unknown_review_request.clone().unwrap();view.review_unknown_command(cx);
        assert!(view.receive_unknown_review(id,&json!({"id":id,"ok":{"reviewed":true,"interruptedCommandId":"old-interrupted-control","durablyAdmitted":true}}),cx));
        assert!(view.cloud_unreviewed_command().is_none());assert!(view.queue_paused,"recovery never silently resumes automatic queue work");
        view.merge_cloud_admissions(&[]);view.merge_cloud_admissions(&rows);assert!(view.cloud_unreviewed_command().is_none(),"late old history cannot undo confirmed review");
    }).unwrap();
    let mut actions = vec![];
    while let Ok(frame) = outgoing.try_recv() {
        assert_ne!(frame["method"], methods::QUEUE_COMMAND);
        if frame["method"] == methods::CLOUD_TASK_COMMAND {
            actions.push(frame);
        }
    }
    assert_eq!(actions.len(), 1);
    let request = &actions[0]["params"]["request"];
    assert_eq!(request["sessionId"], "session");
    assert_eq!(
        request["action"],
        json!({"type":"review-unknown","interruptedCommandId":"old-interrupted-control"})
    );
    assert_ne!(request["commandId"], "old-interrupted-control");
}

#[gpui::test]
fn cloud_send_loader_survives_durable_admission_and_empty_assistant_until_real_response(
    cx: &mut gpui::TestAppContext,
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
    window.update(cx, |view, _, cx| {
        view.cloud.guest = true;
        view.cloud.cached = true;
        view.cloud.guest_workspace = Some("workspace".into());
        view.selected = Some(json!({"id":"session","deviceId":"cloud:workspace","cwd":"."}));
        view.cloud.guest_status = json!({"state":"ready","commandDeliveryMode":"server-admitted-v2","commandAdmissionVersion":2,"commandApiAuthorized":true,"taskCommandCanEdit":true});
        view.cloud.cached_run_selection = Some(("workspace".into(), "session".into(), serde_json::from_value(json!({"harness":"codex","model":"gpt-5.3-codex","reasoning":null,"modelOptions":{},"sandbox":"danger-full-access"})).unwrap()));
        view.input.update(cx, |input, cx| input.set_value(String::from("Run this task"), cx));
    }).unwrap();
    window.update(cx, |view, _, cx| {
        view.submit_saved_cloud_command(cx);
        let (&id, draft) = view.send_draft.iter().next().unwrap();
        let message = draft.message_id.clone().unwrap();
        assert!(view.cloud_preparation_progress().busy, "pending Send must have a loader before a receipt exists");
        view.receive(json!({"id":id,"ok":{"sent":true,"durablyAdmitted":true,"commandId":"command"}}), cx);
        assert!(view.send_draft.is_empty());
        assert!(view.cloud_preparation_progress().busy, "durable receipt bridges the history delivery gap");
        let admissions = vec![json!({"sessionId":"session","commandId":"command","messageId":message,"text":"Run this task","attachments":[],"createdAt":1,"executionState":"starting"})];
        view.merge_cloud_admissions(&admissions);
        let mut placeholder: SessionMessageEntry = serde_json::from_value(json!({"id":"reply","role":"assistant","parts":[{"kind":"text","id":"reply-text","text":""}],"createdAt":2,"deviceId":"runtime"})).unwrap();
        view.messages.push(placeholder.clone());
        assert!(view.cloud_preparation_progress().busy);
        placeholder.parts = vec![zuse_model::MessagePart::Text { id:"reply-text".into(), text:"The task is running.".into() }];
        *view.messages.last_mut().unwrap() = placeholder;
        assert!(!view.cloud_preparation_progress().visible, "real response takes over from preparation loader");
        view.messages.pop();
        let mut completed = admissions;
        completed[0]["executionState"] = json!("completed");
        view.merge_cloud_admissions(&completed);
        assert!(view.cloud.response_wait.is_none());
        assert!(!view.cloud_preparation_progress().visible, "terminal work cannot leave a stuck spinner");
    }).unwrap();
}
