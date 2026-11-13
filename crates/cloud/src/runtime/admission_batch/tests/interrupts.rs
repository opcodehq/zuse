//! Real sealed admission and runtime lease races; no provider or model fixtures.
use super::*;
use std::sync::Arc;
use zuse_protocol::cloud_task_commands as w;
fn access() -> crate::access::Access {
    crate::access::Access {
        actor: crate::auth::Principal {
            account_id: "user-test".into(),
        },
        owner_id: "user-test".into(),
        scope: crate::access::Scope::Personal,
        membership: None,
    }
}
fn stop(id: &str, cid: &str, turn: Option<&str>) -> w::Request {
    serde_json::from_value(json!({"commandId":cid,"sessionId":format!("session-{id}"),"action":{"type":"submit","messageId":format!("control-mid:{cid}"),"payload":{"kind":"interrupt"},"basedOn":turn.map(|v|json!({"turnId":v}))}})).unwrap()
}
fn run(id: &str, cid: &str) -> w::Request {
    serde_json::from_value(json!({"commandId":cid,"sessionId":format!("session-{id}"),"action":{"type":"submit","messageId":format!("mid:{cid}"),"payload":{"kind":"run","messageId":format!("mid:{cid}"),"request":{"prompt":"An explicit later user message","harness":"codex","model":"fixture","cwd":".","sandbox":"workspace-write"}}}})).unwrap()
}
async fn online(pool: &PgPool, id: &str, now: i64) {
    let config = json!({"runtimeBootstrapReceipt":{"workspaceId":id,"generation":1,"gatewayEpoch":1},"runtimeCredentialExpiresAtMs":now+3600000,"runtimeCredentialKeyThumbprint":"key","runtimeSigningKeyThumbprint":"sign","rustStorageEnrollment":{"phase":"committed","generation":1,"sourceSandboxId":format!("machine-{id}"),"storageIncarnationId":format!("disk:{id}"),"credentialKeyThumbprint":"key","signingKeyThumbprint":"sign","capabilities":["rust-session-commands-v1",zuse_protocol::cloud_task_commands::CAPABILITY]}});
    sqlx::query("UPDATE api_cloud_workspaces SET state='ready',desired_state='ready',runtime_state='online',provider_sandbox_id=$2,runtime_credential_hash=$3,request_config=request_config||$4 WHERE workspace_id=$1").bind(id).bind(format!("machine-{id}")).bind(hash("stop-runtime-fixture")).bind(config).execute(pool).await.unwrap();
}
fn lease(id: &str, controls: bool) -> zuse_protocol::cloud_commands::LeaseRequest {
    zuse_protocol::cloud_commands::LeaseRequest {
        control_only: controls,
        max_items: Some(4),
        storage_incarnation_id: format!("disk:{id}"),
        wait_ms: Some(0),
        after_revision: None,
    }
}
pub(super) async fn native(
    control: &RuntimeControl,
    pool: &PgPool,
    id: &str,
    cid: &str,
) -> zuse_protocol::cloud_commands::CloudSessionCommand {
    let envelope: Value = sqlx::query_scalar(
        "SELECT envelope FROM zuse_rust_mailbox_commands WHERE workspace_id=$1 AND command_id=$2",
    )
    .bind(id)
    .bind(cid)
    .fetch_one(pool)
    .await
    .unwrap();
    let wrapped: String = sqlx::query_scalar(
        "SELECT wrapped_transcript_key FROM api_cloud_workspaces WHERE workspace_id=$1",
    )
    .bind(id)
    .fetch_one(pool)
    .await
    .unwrap();
    let raw = control
        .master
        .open_transcript_key("user-test", id, &wrapped)
        .unwrap();
    let key = zuse_cloud_runtime::mailbox::CommandKey::parse(&raw).unwrap();
    let envelope = serde_json::from_value(envelope).unwrap();
    serde_json::from_slice(&key.decrypt(&envelope).unwrap()).unwrap()
}
pub(super) async fn exercise<F: Fn(&str) -> Value>(
    control: &Arc<RuntimeControl>,
    pool: &PgPool,
    _access: &crate::access::Access,
    now: i64,
    prepare: &F,
) {
    steer_inference_remains_active(control, pool, now, prepare).await;
    guest_refusal_fails_without_holding_the_conversation(control, pool, now, prepare).await;
    // A persisted v2 task cannot become runnable through a v1-only guest.
    let gated = "missing-command-cap";
    sqlx::query_scalar::<_, Value>(WRITE_V2.as_str())
        .bind(prepare(gated))
        .fetch_one(pool)
        .await
        .unwrap();
    online(pool, gated, now).await;
    sqlx::query("UPDATE api_cloud_workspaces SET request_config=jsonb_set(jsonb_set(request_config,'{rustStorageEnrollment,capabilities}','[\"rust-session-commands-v1\"]'::jsonb),'{rustStorageEnrollment,enrollmentId}','\"cap-enrollment\"'::jsonb) WHERE workspace_id=$1").bind(gated).execute(pool).await.unwrap();
    let page = control
        .command_lease(gated, "stop-runtime-fixture", &lease(gated, false), now)
        .await
        .unwrap();
    assert!(
        page.leases.is_empty(),
        "old guest must not receive canonical Run envelopes"
    );
    let ready = zuse_protocol::cloud::CloudRuntimeReady {
        phase: "agent-started".into(),
        launch_command_id: Some(format!("launch:{gated}")),
        session_head_version: Some(0),
    };
    assert_eq!(
        control
            .ready_launch(gated, "stop-runtime-fixture", ready, now)
            .await
            .unwrap_err()
            .code,
        "cloud_task_runtime_update_required"
    );
    let request = zuse_protocol::cloud::CloudStorageEnrollment {
        phase: zuse_protocol::cloud::CloudStoragePhase::Commit,
        runtime_generation: 1,
        gateway_epoch: 1,
        storage_incarnation_id: format!("disk:{gated}"),
        enrollment_id: Some("cap-enrollment".into()),
        capabilities: Some(vec!["rust-session-commands-v1".into()]),
    };
    assert_eq!(
        control
            .enroll_storage(gated, "stop-runtime-fixture", request, now)
            .await
            .unwrap_err()
            .code,
        "cloud_task_runtime_update_required"
    );
    let request = zuse_protocol::cloud::CloudStorageEnrollment {
        phase: zuse_protocol::cloud::CloudStoragePhase::Commit,
        runtime_generation: 1,
        gateway_epoch: 1,
        storage_incarnation_id: format!("disk:{gated}"),
        enrollment_id: Some("cap-enrollment".into()),
        capabilities: Some(vec![
            "rust-session-commands-v1".into(),
            zuse_protocol::cloud_task_commands::CAPABILITY.into(),
        ]),
    };
    control
        .enroll_storage(gated, "stop-runtime-fixture", request, now)
        .await
        .unwrap();
    assert_eq!(
        control
            .command_lease(gated, "stop-runtime-fixture", &lease(gated, false), now)
            .await
            .unwrap()
            .leases
            .len(),
        1
    );
    // Stop before computer preparation/first lease must cancel the admitted
    // initial command, without deleting its user message or any future queue.
    let id = "stop-prelease";
    sqlx::query_scalar::<_, Value>(WRITE_V2.as_str())
        .bind(prepare(id))
        .fetch_one(pool)
        .await
        .unwrap();
    let queued = |cid: &str, qid: &str| {
        serde_json::from_value(json!({"commandId":cid,"sessionId":format!("session-{id}"),"action":{"type":"queue","queueId":qid,"messageId":format!("mid:{qid}"),"input":{"text":"Preserve queued user content"},"holdForTurnEnd":true}})).unwrap()
    };
    control
        .task_command_submit(id, &access(), queued("future-queue", "future-q"), now)
        .await
        .unwrap();
    control
        .task_command_submit(id, &access(), queued("preserved-queue", "preserved-q"), now)
        .await
        .unwrap();
    let stopped = control
        .task_command_submit(id, &access(), stop(id, "stop-before-ready", None), now)
        .await
        .unwrap();
    assert_eq!(stopped["cancelledBeforeDispatch"], true);
    assert_eq!(stopped["interruptCommandId"], format!("launch:{id}"));
    let first = native(control, pool, id, "stop-before-ready").await;
    let bound = first.runtime_binding.unwrap();
    assert_eq!(
        bound.interrupt_command_id.as_deref(),
        Some("launch:stop-prelease")
    );
    assert_eq!(
        bound.interrupt_message_id.as_deref(),
        Some("turn:stop-prelease")
    );
    let (state,ever):(String,bool)=sqlx::query_as("SELECT e.state,c.ever_leased FROM zuse_rust_task_command_execution e JOIN zuse_rust_mailbox_commands c USING(workspace_id,command_id) WHERE e.workspace_id=$1 AND e.command_id=$2").bind(id).bind(format!("launch:{id}")).fetch_one(pool).await.unwrap();
    assert_eq!(state, "cancelled");
    assert!(!ever);
    let queue = control
        .task_command_queue(id, &access(), &format!("session-{id}"))
        .await
        .unwrap();
    assert_eq!(queue["paused"], true);
    assert_eq!(queue["items"].as_array().unwrap().len(), 2);
    control.dispatch_task_drain(id, Some(&format!("session-{id}")));
    tokio::time::sleep(std::time::Duration::from_millis(30)).await;
    let pending:i64=sqlx::query_scalar("SELECT count(*) FROM zuse_rust_task_command_queue WHERE workspace_id=$1 AND state='queued'").bind(id).fetch_one(pool).await.unwrap();
    assert_eq!(
        pending, 2,
        "Stop must pause rather than drop/drain future queued content"
    );
    online(pool, id, now).await;
    let page = control
        .command_lease(id, "stop-runtime-fixture", &lease(id, false), now)
        .await
        .unwrap();
    assert!(
        page.leases
            .iter()
            .all(|v| v.command.command_id != "launch:stop-prelease")
    );
    assert!(
        page.leases
            .iter()
            .any(|v| v.command.command_id == "stop-before-ready")
    );

    // A deliberate new Send is allowed without implicitly resuming the rest.
    control
        .task_command_submit(id, &access(), run(id, "explicit-send-after-stop"), now)
        .await
        .unwrap();
    assert_eq!(
        control
            .task_command_queue(id, &access(), &format!("session-{id}"))
            .await
            .unwrap()["paused"],
        true
    );
    let send_now: w::Request=serde_json::from_value(json!({"commandId":"selected-send-now","sessionId":format!("session-{id}"),"action":{"type":"send-now","queueId":"future-q","revision":1}})).unwrap();
    control
        .task_command_submit(id, &access(), send_now, now)
        .await
        .unwrap();
    let queue = control
        .task_command_queue(id, &access(), &format!("session-{id}"))
        .await
        .unwrap();
    assert_eq!(
        queue["paused"], true,
        "SendNow preserves a prior pause for other queued content"
    );
    assert_eq!(queue["items"].as_array().unwrap().len(), 1);
    assert_eq!(queue["items"][0]["id"], "preserved-q");
    let resume:w::Request=serde_json::from_value(json!({"commandId":"explicit-resume-queue","sessionId":format!("session-{id}"),"action":{"type":"pause-queue","paused":false}})).unwrap();
    control
        .task_command_submit(id, &access(), resume, now)
        .await
        .unwrap();
    assert_eq!(
        control
            .task_command_queue(id, &access(), &format!("session-{id}"))
            .await
            .unwrap()["paused"],
        false
    );

    // Stop holds the production workspace/command transaction first; a real
    // runtime lease blocks, then sees only the Stop once cancellation commits.
    let id = "stop-wins-claim";
    sqlx::query_scalar::<_, Value>(WRITE_V2.as_str())
        .bind(prepare(id))
        .fetch_one(pool)
        .await
        .unwrap();
    online(pool, id, now).await;
    let mut tx = pool.begin().await.unwrap();
    let (row, _, _) = locked_workspace_with(&mut tx, id, WorkspaceLock::Command)
        .await
        .unwrap();
    control
        .task_command_submit_tx(
            &mut tx,
            &row,
            &access(),
            &stop(id, "stop-holds-lock", None),
            now,
        )
        .await
        .unwrap();
    let mut pending = tokio::spawn({
        let control = control.clone();
        async move {
            control
                .command_lease(id, "stop-runtime-fixture", &lease(id, false), now)
                .await
        }
    });
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(20), &mut pending)
            .await
            .is_err(),
        "real runtime claim must wait for Stop transaction"
    );
    tx.commit().await.unwrap();
    let page = pending.await.unwrap().unwrap();
    assert!(
        page.leases
            .iter()
            .all(|v| v.command.command_id != "launch:stop-wins-claim")
    );

    // Lease wins first. Concurrent Stop/new-Run admission must retain uncertainty
    // for that exact initial CID; it cannot cancel the later accepted prompt.
    let id = "claim-wins-stop";
    sqlx::query_scalar::<_, Value>(WRITE_V2.as_str())
        .bind(prepare(id))
        .fetch_one(pool)
        .await
        .unwrap();
    online(pool, id, now).await;
    let first = control
        .command_lease(id, "stop-runtime-fixture", &lease(id, false), now)
        .await
        .unwrap();
    assert_eq!(first.leases.len(), 1);
    let current_access = access();
    let (stopped, later) = tokio::join!(
        control.task_command_submit(id, &current_access, stop(id, "stop-inflight", None), now),
        control.task_command_submit(id, &current_access, run(id, "later-accepted"), now)
    );
    assert_eq!(stopped.unwrap()["cancelledBeforeDispatch"], false);
    later.unwrap();
    let original:String=sqlx::query_scalar("SELECT state FROM zuse_rust_task_command_execution WHERE workspace_id=$1 AND command_id=$2").bind(id).bind(format!("launch:{id}")).fetch_one(pool).await.unwrap();
    assert_eq!(
        original, "queued",
        "a lease is not proof of cancellation/completion"
    );
    let page = control
        .command_lease(id, "stop-runtime-fixture", &lease(id, true), now)
        .await
        .unwrap();
    assert!(
        page.leases
            .iter()
            .any(|v| v.command.command_id == "stop-inflight")
    );
    let old = native(control, pool, id, "stop-inflight")
        .await
        .runtime_binding
        .unwrap();
    assert_eq!(
        old.interrupt_command_id.as_deref(),
        Some("launch:claim-wins-stop")
    );
    assert_eq!(
        old.interrupt_message_id.as_deref(),
        Some("turn:claim-wins-stop")
    );
    let later:String=sqlx::query_scalar("SELECT state FROM zuse_rust_task_command_execution WHERE workspace_id=$1 AND command_id='later-accepted'").bind(id).fetch_one(pool).await.unwrap();
    assert_eq!(later, "queued");

    // Fixture supplies factual prior completion and a new native turn. A Stop
    // explicitly based on the old turn must not fall back to that newer Run.
    sqlx::query("UPDATE zuse_rust_task_command_execution SET state='completed',turn_id='old-native-turn' WHERE workspace_id=$1 AND command_id=$2").bind(id).bind(format!("launch:{id}")).execute(pool).await.unwrap();
    sqlx::query("UPDATE zuse_rust_mailbox_commands SET state='applied' WHERE workspace_id=$1 AND command_id IN ($2,'stop-inflight')").bind(id).bind(format!("launch:{id}")).execute(pool).await.unwrap();
    let next = control
        .command_lease(id, "stop-runtime-fixture", &lease(id, false), now)
        .await
        .unwrap();
    assert!(
        next.leases
            .iter()
            .any(|v| v.command.command_id == "later-accepted")
    );
    sqlx::query("UPDATE zuse_rust_task_command_execution SET state='running',turn_id='new-native-turn' WHERE workspace_id=$1 AND command_id='later-accepted'").bind(id).execute(pool).await.unwrap();
    let resume:w::Request=serde_json::from_value(json!({"commandId":"resume-before-stale-test","sessionId":format!("session-{id}"),"action":{"type":"pause-queue","paused":false}})).unwrap();
    control
        .task_command_submit(id, &access(), resume, now)
        .await
        .unwrap();
    assert_eq!(
        control
            .task_command_submit(
                id,
                &access(),
                stop(id, "stale-stop", Some("old-native-turn")),
                now
            )
            .await
            .unwrap_err()
            .code,
        "cloud_task_interrupt_target_changed"
    );
    let later:String=sqlx::query_scalar("SELECT state FROM zuse_rust_task_command_execution WHERE workspace_id=$1 AND command_id='later-accepted'").bind(id).fetch_one(pool).await.unwrap();
    assert_eq!(later, "running");
    let absent:i64=sqlx::query_scalar("SELECT count(*) FROM zuse_rust_mailbox_commands WHERE workspace_id=$1 AND command_id='stale-stop'").bind(id).fetch_one(pool).await.unwrap();
    assert_eq!(absent, 0);
    assert_eq!(
        control
            .task_command_queue(id, &access(), &format!("session-{id}"))
            .await
            .unwrap()["paused"],
        false,
        "rejected stale Stop cannot pause a newer turn's queue"
    );
    assert_eq!(
        native(control, pool, id, "stop-inflight")
            .await
            .runtime_binding
            .unwrap()
            .interrupt_command_id,
        old.interrupt_command_id,
        "late old control remains permanently bound to original Run"
    );
    // A real uncertain control ACK must be visible and explicitly reviewable
    // even though it has no ordinary user-message ID. Review never redispatches.
    let id = "stop-unknown-review";
    sqlx::query_scalar::<_, Value>(WRITE_V2.as_str())
        .bind(prepare(id))
        .fetch_one(pool)
        .await
        .unwrap();
    online(pool, id, now).await;
    control
        .command_lease(id, "stop-runtime-fixture", &lease(id, false), now)
        .await
        .unwrap();
    control
        .task_command_submit(id, &access(), stop(id, "uncertain-control", None), now)
        .await
        .unwrap();
    let controls = control
        .command_lease(id, "stop-runtime-fixture", &lease(id, true), now)
        .await
        .unwrap();
    let current = controls
        .leases
        .iter()
        .find(|v| v.command.command_id == "uncertain-control")
        .unwrap();
    let ack = zuse_protocol::cloud_commands::RuntimeAcknowledgment {
        command_id: current.command.command_id.clone(),
        lease_token: current.lease_token.clone(),
        fingerprint: current.command.fingerprint.clone(),
        state: zuse_protocol::cloud_commands::AcknowledgmentState::OutcomeUnknown,
        category: Some("runtime-admission-uncertain".into()),
        result_iv: None,
        result_ciphertext: None,
    };
    control
        .command_ack(id, "stop-runtime-fixture", &ack, now)
        .await
        .unwrap();
    let history = control
        .task_history(&access(), id, &format!("session-{id}"), None)
        .await
        .unwrap();
    let unknown = history["commandAdmissions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["commandId"] == "uncertain-control")
        .expect("unknown control must not be hidden by null message ID");
    assert_eq!(unknown["unknownReviewed"], false);
    assert_eq!(unknown["errorCode"], "delivery-outcome-unknown");
    assert!(unknown["messageId"].is_null());
    let before: i64 =
        sqlx::query_scalar("SELECT count(*) FROM zuse_rust_mailbox_commands WHERE workspace_id=$1")
            .bind(id)
            .fetch_one(pool)
            .await
            .unwrap();
    let review:w::Request=serde_json::from_value(json!({"commandId":"review-control-outcome","sessionId":format!("session-{id}"),"action":{"type":"review-unknown","interruptedCommandId":"uncertain-control"}})).unwrap();
    let accepted = control
        .task_command_submit(id, &access(), review.clone(), now)
        .await
        .unwrap();
    assert_eq!(accepted["reviewed"], true);
    assert_eq!(
        control
            .task_command_submit(id, &access(), review, now)
            .await
            .unwrap(),
        accepted
    );
    let after: i64 =
        sqlx::query_scalar("SELECT count(*) FROM zuse_rust_mailbox_commands WHERE workspace_id=$1")
            .bind(id)
            .fetch_one(pool)
            .await
            .unwrap();
    assert_eq!(
        before, after,
        "review must never create another execution envelope"
    );
    assert_eq!(
        control
            .task_command_queue(id, &access(), &format!("session-{id}"))
            .await
            .unwrap()["paused"],
        true
    );
    let state:String=sqlx::query_scalar("SELECT state FROM zuse_rust_mailbox_commands WHERE workspace_id=$1 AND command_id='uncertain-control'").bind(id).fetch_one(pool).await.unwrap();
    assert_eq!(
        state, "outcome-unknown",
        "review is not fabricated completion"
    );
    let history = control
        .task_history(&access(), id, &format!("session-{id}"), None)
        .await
        .unwrap();
    assert!(
        !history["commandAdmissions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v["commandId"] == "uncertain-control" && v["unknownReviewed"] != true)
    );
}

// A guest that refused a message before anything ran (here: an attachment
// that is not one of its uploads) acknowledges Rejected with the reason. The
// message fails with that reason and the next one is admitted and delivered,
// with no unknown outcome to review.
async fn guest_refusal_fails_without_holding_the_conversation<F: Fn(&str) -> Value>(
    control: &Arc<RuntimeControl>,
    pool: &PgPool,
    now: i64,
    prepare: &F,
) {
    let id = "guest-refusal";
    let session = format!("session-{id}");
    sqlx::query_scalar::<_, Value>(WRITE_V2.as_str())
        .bind(prepare(id))
        .fetch_one(pool)
        .await
        .unwrap();
    online(pool, id, now).await;
    let page = control
        .command_lease(id, "stop-runtime-fixture", &lease(id, false), now)
        .await
        .unwrap();
    let refused = page
        .leases
        .iter()
        .find(|v| v.command.kind == "zuse.session.run")
        .expect("the launch Run is delivered");
    let refused_id = refused.command.command_id.clone();
    control
        .command_ack(
            id,
            "stop-runtime-fixture",
            &zuse_protocol::cloud_commands::RuntimeAcknowledgment {
                command_id: refused_id.clone(),
                lease_token: refused.lease_token.clone(),
                fingerprint: refused.command.fingerprint.clone(),
                state: zuse_protocol::cloud_commands::AcknowledgmentState::Rejected,
                category: Some("cloud_command_upload_outside_uploads".into()),
                result_iv: None,
                result_ciphertext: None,
            },
            now,
        )
        .await
        .unwrap();
    let (state, code, delivery): (String, String, String) = sqlx::query_as("SELECT e.state,e.error_code,c.state FROM zuse_rust_task_command_execution e JOIN zuse_rust_mailbox_commands c USING(workspace_id,command_id) WHERE e.workspace_id=$1 AND e.command_id=$2").bind(id).bind(&refused_id).fetch_one(pool).await.unwrap();
    assert_eq!(
        (state.as_str(), code.as_str(), delivery.as_str()),
        ("failed", "cloud_command_upload_outside_uploads", "rejected")
    );
    let status = control
        .task_command_status(id, &access(), &refused_id)
        .await
        .unwrap();
    assert_eq!(status["executionState"], "failed");
    assert_eq!(status["errorCode"], "cloud_command_upload_outside_uploads");
    // The next message on the same conversation is admitted, not held for
    // review, and is the next Run delivered.
    control
        .task_command_submit(id, &access(), run(id, "after-refusal"), now)
        .await
        .expect("a refused message must not hold the conversation");
    let next = control
        .command_lease(id, "stop-runtime-fixture", &lease(id, false), now)
        .await
        .unwrap();
    assert!(
        next.leases
            .iter()
            .any(|v| v.command.command_id == "after-refusal"),
        "the next message is delivered after a definitive refusal"
    );
    let history = control
        .task_history(&access(), id, &session, None)
        .await
        .unwrap();
    let rows = history["commandAdmissions"].as_array().unwrap();
    let row = rows
        .iter()
        .find(|v| v["commandId"] == refused_id.as_str())
        .unwrap();
    assert_eq!(row["executionState"], "failed");
    assert_eq!(row["errorCode"], "cloud_command_upload_outside_uploads");
    assert!(
        !rows
            .iter()
            .any(|v| v["executionState"] == "outcome-unknown")
    );
}

// Native steering transfers inference ownership onto a control transport lane.
// Exercise real encrypted admission, runtime lease and authenticated progress.
async fn steer_inference_remains_active<F: Fn(&str) -> Value>(
    control: &Arc<RuntimeControl>,
    pool: &PgPool,
    now: i64,
    prepare: &F,
) {
    use zuse_protocol::cloud_task_commands::{ExecutionState, Progress};
    let id = "steer-inference";
    let session = format!("session-{id}");
    sqlx::query_scalar::<_, Value>(WRITE_V2.as_str())
        .bind(prepare(id))
        .fetch_one(pool)
        .await
        .unwrap();
    online(pool, id, now).await;
    // Prior Run has already transferred ownership at the canonical steer boundary.
    sqlx::query(
        "UPDATE zuse_rust_task_command_execution SET state='cancelled' WHERE workspace_id=$1",
    )
    .bind(id)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query("UPDATE zuse_rust_mailbox_commands SET state='applied',ever_leased=true WHERE workspace_id=$1").bind(id).execute(pool).await.unwrap();
    let submit = |cid: &str, action: Value| {
        serde_json::from_value(json!({"commandId":cid,"sessionId":session,"action":action}))
            .unwrap()
    };
    control.task_command_submit(id,&access(),submit("live-steer",json!({"type":"submit","messageId":"steer-mid","payload":{"kind":"steer","prompt":"Continue with extra context","messageId":"steer-mid"}})),now).await.unwrap();
    let leased = control
        .command_lease(id, "stop-runtime-fixture", &lease(id, false), now)
        .await
        .unwrap();
    assert_eq!(leased.leases.len(), 1);
    assert_eq!(leased.leases[0].command.command_id, "live-steer");
    let lane:String=sqlx::query_scalar("SELECT lane FROM zuse_rust_mailbox_commands WHERE workspace_id=$1 AND command_id='live-steer'").bind(id).fetch_one(pool).await.unwrap();
    assert_eq!(
        lane,
        format!("task-control:{session}"),
        "Steer must bypass ordinary Run transport ordering"
    );
    let progress = |revision, state| Progress {
        command_id: "live-steer".into(),
        fingerprint: leased.leases[0].command.fingerprint.clone(),
        storage_incarnation_id: format!("disk:{id}"),
        revision,
        state,
        turn_id: Some("steer-owned-turn".into()),
        error_code: None,
    };
    for (revision, state) in [(1, ExecutionState::Starting), (2, ExecutionState::Running)] {
        control
            .task_command_progress(id, "stop-runtime-fixture", &progress(revision, state), now)
            .await
            .unwrap();
    }
    sqlx::query("UPDATE zuse_rust_mailbox_commands SET state='applied' WHERE workspace_id=$1 AND command_id='live-steer'").bind(id).execute(pool).await.unwrap();
    let stopped = control
        .task_command_submit(
            id,
            &access(),
            stop(id, "stop-steer", Some("steer-owned-turn")),
            now,
        )
        .await
        .unwrap();
    assert_eq!(
        stopped["interruptCommandId"], "live-steer",
        "Stop must target live inference owned by Steer"
    );
    // Stop admission alone does not terminalize inference. Clear queue pause only
    // to exercise the independent held-row busy guard (no fake execution ACK).
    control
        .task_command_submit(
            id,
            &access(),
            submit(
                "unpause-steer",
                json!({"type":"pause-queue","paused":false}),
            ),
            now,
        )
        .await
        .unwrap();
    let queue = |qid: &str| json!({"type":"queue","queueId":qid,"messageId":format!("mid:{qid}"),"input":{"text":"Next context"},"holdForTurnEnd":true});
    control
        .task_command_submit(
            id,
            &access(),
            submit("held-steer", queue("held-steer-q")),
            now,
        )
        .await
        .unwrap();
    control.work_task_drain_once(now).await.unwrap();
    let actor = access();
    let (drain, stop_again) = tokio::join!(
        control.work_task_drain_once(now),
        control.task_command_submit(
            id,
            &actor,
            stop(id, "parallel-stop-steer", Some("steer-owned-turn")),
            now
        )
    );
    drain.unwrap();
    assert_eq!(stop_again.unwrap()["interruptCommandId"], "live-steer");
    let state:String=sqlx::query_scalar("SELECT state FROM zuse_rust_task_command_queue WHERE workspace_id=$1 AND queue_id='held-steer-q'").bind(id).fetch_one(pool).await.unwrap();
    assert_eq!(
        state, "queued",
        "held queue cannot promote while Steer inference runs"
    );
    control
        .task_command_submit(
            id,
            &access(),
            submit("next-steer-q", queue("next-steer-q")),
            now,
        )
        .await
        .unwrap();
    control
        .task_command_submit(
            id,
            &access(),
            submit(
                "next-steer",
                json!({"type":"steer-now","queueId":"next-steer-q","revision":1}),
            ),
            now,
        )
        .await
        .unwrap();
    let next = native(control, pool, id, "next-steer").await;
    assert_eq!(
        next.based_on.unwrap().turn_id.as_deref(),
        Some("steer-owned-turn")
    );
    control
        .task_command_submit(id, &access(), run(id, "blocked-run"), now)
        .await
        .unwrap();
    let eligible = |guard: &str| {
        format!(
            "SELECT count(*) FROM zuse_rust_mailbox_commands c WHERE c.workspace_id='{id}' AND c.command_id='blocked-run' {guard}"
        )
    };
    let count: i64 = sqlx::query_scalar(&eligible(
        crate::runtime::task_commands::dispatch::EXECUTION_GUARD,
    ))
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(
        count, 0,
        "ordinary Run cannot dispatch over live Steer inference"
    );
    // Unconsumed extra steering is terminalized only as fixture cleanup; it must
    // not become an unrelated queued control barrier after the real turn ends.
    sqlx::query("UPDATE zuse_rust_task_command_execution SET state='completed' WHERE workspace_id=$1 AND command_id='next-steer'").bind(id).execute(pool).await.unwrap();
    control
        .task_command_progress(
            id,
            "stop-runtime-fixture",
            &progress(3, ExecutionState::Completed),
            now + 1,
        )
        .await
        .unwrap();
    let count: i64 = sqlx::query_scalar(&eligible(
        crate::runtime::task_commands::dispatch::EXECUTION_GUARD,
    ))
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(
        count, 1,
        "completed Steer and message-less pending controls do not block Run"
    );
    sqlx::query("UPDATE zuse_rust_task_command_execution SET state='completed' WHERE workspace_id=$1 AND command_id='blocked-run'").bind(id).execute(pool).await.unwrap();
    control
        .task_command_submit(
            id,
            &access(),
            submit(
                "final-unpause",
                json!({"type":"pause-queue","paused":false}),
            ),
            now + 2,
        )
        .await
        .unwrap();
    control.work_task_drain_once(now + 40_000).await.unwrap();
    let state:String=sqlx::query_scalar("SELECT state FROM zuse_rust_task_command_queue WHERE workspace_id=$1 AND queue_id='held-steer-q'").bind(id).fetch_one(pool).await.unwrap();
    assert_eq!(
        state, "claimed",
        "factual Steer completion releases held queue once"
    );
}
