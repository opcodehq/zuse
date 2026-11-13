//! A message that has to wait for its computer (an in-place runtime update,
//! or Update computer) is saved in the conversation's queue on the server
//! instead of being refused, and is sent once the computer can take it:
//!
//! ```text
//! send (queueWhileWaiting) ─▶ queued { waitingFor } ─▶ update commits / recreation ready
//!                                                   ─▶ drained to the computer
//! ```
//!
//! Runs after `runtime_updates`, whose releases it reuses.
use super::runtime_outdated::{computer, enqueued, run};
use super::runtime_updates::{CREDENTIAL, FILE, attach_runtime, attestation, sha};
use super::*;
use crate::runtime::task_commands::Submit;
use zuse_protocol::{cloud_runtime_updates as u, cloud_task_commands as w};

const QUEUE: Submit = Submit {
    queue_while_waiting: true,
};

async fn queue_page(control: &RuntimeControl, access: &crate::access::Access, id: &str) -> Value {
    control
        .task_command_queue(id, access, &format!("session:{id}"))
        .await
        .unwrap()
}

async fn last_error(pool: &PgPool, id: &str) -> Option<String> {
    sqlx::query_scalar("SELECT last_error FROM zuse_rust_task_command_drains WHERE workspace_id=$1")
        .bind(id)
        .fetch_optional(pool)
        .await
        .unwrap()
        .flatten()
}

async fn queue_state(pool: &PgPool, id: &str, queue: &str) -> String {
    sqlx::query_scalar(
        "SELECT state FROM zuse_rust_task_command_queue WHERE workspace_id=$1 AND queue_id=$2",
    )
    .bind(id)
    .bind(queue)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn ran(pool: &PgPool, id: &str, message: &str) -> bool {
    sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM zuse_rust_task_command_execution WHERE workspace_id=$1 AND message_id=$2)")
        .bind(id)
        .bind(message)
        .fetch_one(pool)
        .await
        .unwrap()
}

/// The fixture's first message finished, so the conversation is free.
async fn first_turn_done(pool: &PgPool, id: &str) {
    sqlx::query(
        "UPDATE zuse_rust_task_command_execution SET state='completed' WHERE workspace_id=$1",
    )
    .bind(id)
    .execute(pool)
    .await
    .unwrap();
}

async fn set_capabilities(owner: &PgPool, id: &str, capabilities: &[Value]) {
    sqlx::query("UPDATE api_cloud_workspaces SET request_config=jsonb_set(request_config,'{runtimeBootstrapReceipt,capabilities}',$2) WHERE workspace_id=$1")
        .bind(id)
        .bind(json!(capabilities))
        .execute(owner)
        .await
        .unwrap();
}

pub(super) async fn exercise(
    control: &std::sync::Arc<RuntimeControl>,
    owner: &PgPool,
    pool: &PgPool,
    access: &crate::access::Access,
    master: &crate::crypto::MasterKey,
    plan: impl Fn(&str) -> Value,
    now: i64,
) {
    let (old, new) = (sha('1'), sha('2'));
    let old_caps = vec![
        json!("rust-rpc-v1"),
        json!("rust-session-commands-v1"),
        json!(w::CAPABILITY),
        json!(u::CAPABILITY),
    ];
    let mut new_caps = old_caps.clone();
    new_caps.push(json!(w::DIRECT_UPLOADS_CAPABILITY));
    new_caps.push(json!(w::MESSAGE_ACCESS_MODE_CAPABILITY));

    // 1. An in-place update: queued, survives a rollback, sent on commit.
    let id = "runtime-wait-update";
    computer(
        control,
        pool,
        access,
        master,
        plan(id),
        Some(old_caps.clone()),
        now,
    )
    .await;
    attach_runtime(owner, id, &old, now).await;
    first_turn_done(pool, id).await;
    let before = enqueued(pool, id).await;
    // An older client (no flag) is still refused, exactly as before.
    let refused = control
        .task_command_submit(id, access, run(id, "old-client", &[FILE], true), now)
        .await
        .unwrap_err();
    assert_eq!(refused.code, u::RUNTIME_UPDATING);
    // Files and full access, which this runtime can't take yet.
    let cid = "wait-update";
    let mid = format!("{cid}-message");
    let request = run(id, cid, &[FILE], true);
    let accepted = control
        .task_command_submit_with(id, access, request.clone(), QUEUE, now)
        .await
        .unwrap();
    assert_eq!(accepted["durablyAdmitted"], true);
    assert_eq!(accepted["commandId"], cid);
    assert_eq!(accepted["state"], "queued");
    assert_eq!(accepted["waitingFor"], u::RUNTIME_UPDATING);
    assert_eq!(accepted["id"], mid, "its queued row is the sent message");
    assert_eq!(accepted["messageId"], mid);
    // A lost reply retried is the same queued message.
    assert_eq!(
        control
            .task_command_submit_with(id, access, request, QUEUE, now)
            .await
            .unwrap(),
        accepted
    );
    assert_eq!(
        enqueued(pool, id).await,
        before,
        "nothing reached the computer"
    );
    // The update is demanded now, as for a refusal.
    let urgency: String =
        sqlx::query_scalar("SELECT urgency FROM zuse_rust_runtime_updates WHERE workspace_id=$1")
            .bind(id)
            .fetch_one(pool)
            .await
            .unwrap();
    assert_eq!(urgency, "now");
    // Every device sees it queued, with everything it was sent with.
    let page = queue_page(control, access, id).await;
    assert_eq!(page["queueError"], u::RUNTIME_UPDATING);
    let item = &page["items"][0];
    assert_eq!(item["id"], mid);
    assert_eq!(item["text"], "Look at this");
    assert_eq!(item["computerFiles"], json!([FILE]));
    assert_eq!(
        item["input"]["runSelection"]["sandbox"],
        "danger-full-access"
    );
    assert_eq!(item["input"]["runSelection"]["model"], "fixture");
    assert_eq!(item["input"]["runSelection"]["harness"], "codex");

    // While it updates, the drain keeps it.
    assert!(control.work_task_drain_for(id, now + 1).await.unwrap());
    assert_eq!(queue_state(pool, id, &mid).await, "queued");
    assert_eq!(
        last_error(pool, id).await.as_deref(),
        Some(u::RUNTIME_UPDATING)
    );

    // A rollback loses nothing; the next attempt asks for the update again.
    let first = control
        .runtime_update_offer(id, CREDENTIAL, now + 2)
        .await
        .unwrap()
        .directive
        .unwrap();
    assert_eq!(first.urgency, u::Urgency::Now);
    control
        .runtime_update_attest(
            id,
            CREDENTIAL,
            attestation(
                u::Outcome::RolledBack,
                Some(first.update_id.clone()),
                &old,
                &old_caps,
            ),
            now + 3,
        )
        .await
        .unwrap();
    assert!(control.work_task_drain_for(id, now + 40_000).await.unwrap());
    assert_eq!(queue_state(pool, id, &mid).await, "queued");
    assert_eq!(
        last_error(pool, id).await.as_deref(),
        Some(u::RUNTIME_UPDATING)
    );
    let retry: (String, String) =
        sqlx::query_as("SELECT urgency,state FROM zuse_rust_runtime_updates WHERE workspace_id=$1")
            .bind(id)
            .fetch_one(pool)
            .await
            .unwrap();
    assert_eq!(retry, ("now".into(), "pending".into()));

    // Commit: it goes out at once, not at the next retry.
    let second = control
        .runtime_update_offer(id, CREDENTIAL, now + 40_001)
        .await
        .unwrap()
        .directive
        .unwrap();
    control
        .runtime_update_attest(
            id,
            CREDENTIAL,
            attestation(
                u::Outcome::Committed,
                Some(second.update_id),
                &new,
                &new_caps,
            ),
            now + 40_002,
        )
        .await
        .unwrap();
    assert!(control.work_task_drain_for(id, now + 40_002).await.unwrap());
    assert_eq!(queue_state(pool, id, &mid).await, "claimed");
    assert!(ran(pool, id, &mid).await);
    assert_eq!(enqueued(pool, id).await, before + 1);
    assert_eq!(last_error(pool, id).await, None, "nothing left waiting");
    let page = queue_page(control, access, id).await;
    assert_eq!(page["items"], json!([]));
    assert!(page["queueError"].is_null());

    // 2. No in-place update: queued until Update computer finishes.
    let id = "runtime-wait-recreate";
    let mut without = old_caps.clone();
    without.retain(|c| c != u::CAPABILITY);
    computer(
        control,
        pool,
        access,
        master,
        plan(id),
        Some(without.clone()),
        now,
    )
    .await;
    attach_runtime(owner, id, &old, now).await;
    first_turn_done(pool, id).await;
    let before = enqueued(pool, id).await;
    let mode = "wait-mode";
    let accepted = control
        .task_command_submit_with(id, access, run(id, mode, &[], true), QUEUE, now)
        .await
        .unwrap();
    assert_eq!(accepted["waitingFor"], u::RUNTIME_UPDATE_UNAVAILABLE);
    assert!(control.work_task_drain_for(id, now + 1).await.unwrap());
    assert_eq!(
        last_error(pool, id).await.as_deref(),
        Some(u::RUNTIME_UPDATE_UNAVAILABLE),
        "the app offers Update computer"
    );
    // Update computer starts: every new message waits for it, even a plain one.
    sqlx::query("UPDATE api_cloud_workspaces SET request_config=request_config||'{\"rustRecreation\":{\"operationId\":\"recreate-fixture\",\"phase\":\"restoring\"}}'::jsonb WHERE workspace_id=$1")
        .bind(id)
        .execute(owner)
        .await
        .unwrap();
    let refused = control
        .task_command_submit(id, access, run(id, "old-client-plain", &[], false), now)
        .await
        .unwrap_err();
    assert_eq!(refused.code, u::RECREATING);
    let plain = "wait-plain";
    let mut request = run(id, plain, &[], false);
    // A recreation brings the computer up itself; no wake is attempted.
    request.wake = w::WakePolicy::ExplicitResume;
    let accepted = control
        .task_command_submit_with(id, access, request, QUEUE, now + 2)
        .await
        .unwrap();
    assert_eq!(accepted["waitingFor"], u::RECREATING);
    assert!(accepted.get("wake").is_none());
    assert!(control.work_task_drain_for(id, now + 40_000).await.unwrap());
    assert_eq!(last_error(pool, id).await.as_deref(), Some(u::RECREATING));
    let page = queue_page(control, access, id).await;
    let order: Vec<&str> = page["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["id"].as_str().unwrap())
        .collect();
    assert_eq!(
        order,
        [format!("{mode}-message"), format!("{plain}-message")]
    );
    assert_eq!(enqueued(pool, id).await, before);

    // Ready on a current runtime: the queue resumes and sends them in order.
    sqlx::query("UPDATE api_cloud_workspaces SET request_config=jsonb_set(request_config,'{rustRecreation,phase}','\"ready\"') WHERE workspace_id=$1")
        .bind(id)
        .execute(owner)
        .await
        .unwrap();
    set_capabilities(owner, id, &new_caps).await;
    let mut tx = pool.begin().await.unwrap();
    let (row, _, _) = locked_workspace_with(&mut tx, id, WorkspaceLock::Command)
        .await
        .unwrap();
    RuntimeControl::resume_waiting_queues_tx(&mut tx, &row, now + 40_001)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert!(control.work_task_drain_for(id, now + 40_001).await.unwrap());
    assert_eq!(
        queue_state(pool, id, &format!("{mode}-message")).await,
        "claimed"
    );
    assert_eq!(
        queue_state(pool, id, &format!("{plain}-message")).await,
        "queued"
    );
    assert_eq!(enqueued(pool, id).await, before + 1);
    assert_eq!(last_error(pool, id).await, None);

    // 3. A message that could never run is still refused, not left waiting.
    let mut request = run(id, "wait-bad-model", &[FILE], true);
    if let w::Action::Submit {
        payload: zuse_protocol::session_commands::SessionCommandPayload::Run { request, .. },
        ..
    } = &mut request.action
    {
        request.model = Some("not-installed".into());
    }
    set_capabilities(owner, id, &without).await;
    let refused = control
        .task_command_submit_with(id, access, request, QUEUE, now + 40_002)
        .await
        .unwrap_err();
    assert_eq!(refused.code, "cloud_task_model_not_installed");
}
