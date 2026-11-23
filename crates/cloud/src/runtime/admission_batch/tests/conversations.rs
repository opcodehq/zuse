//! Several conversations on one cloud computer: one registry, one thread and
//! one machine, with each conversation's queue, runs and controls kept apart.
use super::interrupts::{access, lease, native, online};
use super::*;
use std::sync::Arc;
use zuse_protocol::cloud_task_commands as w;

fn request(session: &str, cid: &str, action: Value) -> w::Request {
    serde_json::from_value(json!({"commandId":cid,"sessionId":session,"action":action})).unwrap()
}
fn run(session: &str, cid: &str) -> w::Request {
    request(
        session,
        cid,
        json!({"type":"submit","messageId":format!("mid:{cid}"),"payload":{"kind":"run","messageId":format!("mid:{cid}"),"request":{"prompt":"Work in this conversation","harness":"codex","model":"fixture","cwd":".","sandbox":"workspace-write"}}}),
    )
}
fn stop(session: &str, cid: &str) -> w::Request {
    request(
        session,
        cid,
        json!({"type":"submit","messageId":format!("control-mid:{cid}"),"payload":{"kind":"interrupt"}}),
    )
}
fn queue(session: &str, cid: &str, qid: &str) -> w::Request {
    request(
        session,
        cid,
        json!({"type":"queue","queueId":qid,"messageId":format!("mid:{qid}"),"input":{"text":"After this turn"},"holdForTurnEnd":true}),
    )
}
async fn execution(pool: &PgPool, id: &str, cid: &str) -> (String, String) {
    sqlx::query_as("SELECT session_id,state FROM zuse_rust_task_command_execution WHERE workspace_id=$1 AND command_id=$2")
        .bind(id)
        .bind(cid)
        .fetch_one(pool)
        .await
        .unwrap()
}

pub(super) async fn exercise<F: Fn(&str) -> Value>(
    control: &Arc<RuntimeControl>,
    pool: &PgPool,
    now: i64,
    prepare: &F,
) {
    let id = "two-conversations";
    let first = format!("session-{id}");
    let thread = format!("chat-{id}");
    let second = "second-conversation";
    sqlx::query_scalar::<_, Value>(WRITE_V2.as_str())
        .bind(prepare(id))
        .fetch_one(pool)
        .await
        .unwrap();
    online(pool, id, now).await;
    // The first conversation's launch has run and ended on the computer.
    let launch = control
        .command_lease(id, "stop-runtime-fixture", &lease(id, false), now)
        .await
        .unwrap();
    assert_eq!(launch.leases.len(), 1);
    sqlx::query("UPDATE zuse_rust_task_command_execution SET state='completed' WHERE workspace_id=$1 AND command_id=$2").bind(id).bind(format!("launch:{id}")).execute(pool).await.unwrap();
    sqlx::query("UPDATE zuse_rust_mailbox_commands SET state='applied' WHERE workspace_id=$1 AND command_id=$2").bind(id).bind(format!("launch:{id}")).execute(pool).await.unwrap();

    // A conversation that was never registered is refused, so "+" can never
    // reach the computer through an unknown identity.
    assert_eq!(
        control
            .task_command_submit(id, &access(), run("unregistered", "stray-run"), now)
            .await
            .unwrap_err()
            .code,
        "cloud_task_session_not_registered"
    );

    // "+": a second conversation in the same thread, admitted without a wake.
    let create = request(
        second,
        "create-second",
        json!({"type":"create-session","title":null}),
    );
    let created = control
        .task_command_submit(id, &access(), create.clone(), now + 1)
        .await
        .unwrap();
    assert_eq!(created["sessionId"], second);
    assert_eq!(created["threadId"], thread);
    assert_eq!(
        control
            .task_command_submit(id, &access(), create, now + 1)
            .await
            .unwrap(),
        created,
        "a lost reply replays the same conversation"
    );
    assert_eq!(
        control
            .task_command_submit(
                id,
                &access(),
                request(
                    second,
                    "create-second-again",
                    json!({"type":"create-session","title":null})
                ),
                now + 1,
            )
            .await
            .unwrap_err()
            .code,
        "cloud_task_session_identity_conflict"
    );
    let workspaces: i64 =
        sqlx::query_scalar("SELECT count(*) FROM api_cloud_workspaces WHERE chat_id=$1")
            .bind(&thread)
            .fetch_one(pool)
            .await
            .unwrap();
    assert_eq!(workspaces, 1, "a conversation never creates a computer");
    let listed = control
        .task_command_sessions(id, &access(), None)
        .await
        .unwrap();
    let mut ids = listed["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|chat| {
            assert_eq!(chat["threadId"], thread);
            chat["id"].as_str().unwrap().to_owned()
        })
        .collect::<Vec<_>>();
    ids.sort();
    assert_eq!(ids, vec![second.to_owned(), first.clone()]);
    let empty = control
        .task_history(&access(), id, second, None)
        .await
        .unwrap();
    assert_eq!(empty["source"], "admission");
    assert_eq!(empty["session"]["chat"]["id"], second);
    assert_eq!(empty["session"]["chat"]["threadId"], thread);
    assert_eq!(
        empty["registeredConversations"].as_array().unwrap().len(),
        2
    );

    // Both conversations run at once on the one computer. Only the new one's
    // first Run carries its registration, so the computer can install it.
    control
        .task_command_submit(id, &access(), run(&first, "run-first"), now + 2)
        .await
        .unwrap();
    control
        .task_command_submit(id, &access(), run(second, "run-second"), now + 3)
        .await
        .unwrap();
    let activity: i64 = sqlx::query_scalar(
        "SELECT last_activity_at FROM api_cloud_workspaces WHERE workspace_id=$1",
    )
    .bind(id)
    .fetch_one(pool)
    .await
    .unwrap();
    assert!(
        activity >= now + 3,
        "work in any conversation keeps the computer active"
    );
    let page = control
        .command_lease(id, "stop-runtime-fixture", &lease(id, false), now + 3)
        .await
        .unwrap();
    let mut leased = page
        .leases
        .iter()
        .map(|v| v.command.command_id.clone())
        .collect::<Vec<_>>();
    leased.sort();
    assert_eq!(leased, vec!["run-first", "run-second"]);
    assert!(
        native(control, pool, id, "run-first")
            .await
            .runtime_binding
            .unwrap()
            .session
            .is_none()
    );
    let registration = native(control, pool, id, "run-second")
        .await
        .runtime_binding
        .unwrap()
        .session
        .unwrap();
    assert_eq!(registration.session_id, second);
    assert_eq!(registration.thread_id, thread);
    assert_eq!(execution(pool, id, "run-first").await.0, first);
    assert_eq!(execution(pool, id, "run-second").await.0, second);
    sqlx::query("UPDATE zuse_rust_task_command_execution SET state='running' WHERE workspace_id=$1 AND command_id IN ('run-first','run-second')").bind(id).execute(pool).await.unwrap();

    // Each conversation has its own queue.
    control
        .task_command_submit(
            id,
            &access(),
            queue(&first, "queue-first", "first-q"),
            now + 4,
        )
        .await
        .unwrap();
    let first_queue = control
        .task_command_queue(id, &access(), &first)
        .await
        .unwrap();
    let second_queue = control
        .task_command_queue(id, &access(), second)
        .await
        .unwrap();
    assert_eq!(first_queue["items"].as_array().unwrap().len(), 1);
    assert_eq!(second_queue["items"].as_array().unwrap().len(), 0);

    // Stop in the second conversation targets only its own turn and pauses
    // only its own queue.
    control
        .task_command_submit(id, &access(), stop(second, "stop-second"), now + 5)
        .await
        .unwrap();
    let controls = control
        .command_lease(id, "stop-runtime-fixture", &lease(id, true), now + 5)
        .await
        .unwrap();
    assert!(
        controls
            .leases
            .iter()
            .any(|v| v.command.command_id == "stop-second")
    );
    let binding = native(control, pool, id, "stop-second")
        .await
        .runtime_binding
        .unwrap();
    assert_eq!(binding.interrupt_command_id.as_deref(), Some("run-second"));
    assert!(binding.session.is_none());
    assert_eq!(execution(pool, id, "run-first").await.1, "running");
    assert_eq!(
        control
            .task_command_queue(id, &access(), second)
            .await
            .unwrap()["paused"],
        true
    );
    assert_eq!(
        control
            .task_command_queue(id, &access(), &first)
            .await
            .unwrap()["paused"],
        false,
        "another conversation's Stop never pauses this queue"
    );

    // The first conversation's queued message waits for its own turn only.
    sqlx::query("UPDATE zuse_rust_task_command_execution SET state='cancelled' WHERE workspace_id=$1 AND command_id='run-second'").bind(id).execute(pool).await.unwrap();
    // Each conversation has its own drain; run every one that is due.
    for _ in 0..8 {
        if !control.work_task_drain_for(id, now + 40_000).await.unwrap() {
            break;
        }
    }
    let held: String = sqlx::query_scalar(
        "SELECT state FROM zuse_rust_task_command_queue WHERE workspace_id=$1 AND queue_id='first-q'",
    )
    .bind(id)
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(held, "queued", "held while its own conversation runs");
    sqlx::query("UPDATE zuse_rust_task_command_execution SET state='completed' WHERE workspace_id=$1 AND command_id='run-first'").bind(id).execute(pool).await.unwrap();
    sqlx::query("UPDATE zuse_rust_mailbox_commands SET state='applied' WHERE workspace_id=$1 AND command_id IN ('run-first','run-second','stop-second')").bind(id).execute(pool).await.unwrap();
    for _ in 0..8 {
        if !control.work_task_drain_for(id, now + 80_000).await.unwrap() {
            break;
        }
    }
    let sent: String = sqlx::query_scalar(
        "SELECT state FROM zuse_rust_task_command_queue WHERE workspace_id=$1 AND queue_id='first-q'",
    )
    .bind(id)
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(sent, "claimed");

    // Each conversation's history reads its own admissions.
    let second_history = control
        .task_history(&access(), id, second, None)
        .await
        .unwrap();
    let messages = second_history["commandAdmissions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["commandId"].as_str().unwrap_or_default().to_owned())
        .collect::<Vec<_>>();
    assert!(messages.contains(&"run-second".to_owned()));
    assert!(!messages.contains(&"run-first".to_owned()));
}
