//! Production queue/control handlers under the actual restricted SQL role.
use super::*;
use zuse_protocol::cloud_task_commands as w;
pub(super) async fn exercise(
    control: &std::sync::Arc<RuntimeControl>,
    pool: &PgPool,
    access: &crate::access::Access,
    now: i64,
) {
    let request = |cid: &str, action: w::Action| w::Request {
        command_id: cid.into(),
        session_id: "session:crypto".into(),
        action,
        wake: w::WakePolicy::PreserveSleep,
    };
    let input = |text: &str| w::Input {
        run_selection: None,
        text: text.into(),
        attachments: vec![],
        file_refs: vec!["src/lib.rs".into()],
        skill_refs: vec!["skills/test.md".into()],
        annotations: vec!["Reviewed user context".into()],
        as_goal: false,
    };
    let queued = request(
        "queue-action",
        w::Action::Queue {
            queue_id: "q-one".into(),
            message_id: "mid-one".into(),
            input: input("Final edited text"),
            hold_for_turn_end: true,
        },
    );
    let accepted = control
        .task_command_submit("crypto", access, queued.clone(), now)
        .await
        .unwrap();
    assert_eq!(
        control
            .task_command_submit("crypto", access, queued, now)
            .await
            .unwrap(),
        accepted,
        "lost admission reply keeps same logical action"
    );
    assert!(control.work_task_drain_once(now).await.unwrap());
    let claimed: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM zuse_rust_task_command_queue WHERE state='claimed'",
    )
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(
        claimed, 0,
        "initial accepted Run has no factual terminal yet"
    );
    let editing = control
        .task_command_submit(
            "crypto",
            access,
            request(
                "edit-lease",
                w::Action::BeginEdit {
                    queue_id: "q-one".into(),
                    revision: 1,
                    editor_device_id: "device".into(),
                    editor_instance_id: "editor".into(),
                },
            ),
            now,
        )
        .await
        .unwrap();
    // Fixture supplies a terminal fact; progress state-machine itself is tested
    // through authenticated runtime reports by the execution-progress matrix.
    sqlx::query("UPDATE zuse_rust_task_command_execution SET state='completed' WHERE command_id='launch:crypto'").execute(pool).await.unwrap();
    sqlx::query(
        "UPDATE zuse_rust_mailbox_commands SET state='applied' WHERE command_id='launch:crypto'",
    )
    .execute(pool)
    .await
    .unwrap();
    let later = now + w::EDIT_LEASE_MS + 40_000;
    assert!(control.work_task_drain_once(later).await.unwrap());
    let state: String =
        sqlx::query_scalar("SELECT state FROM zuse_rust_task_command_queue WHERE queue_id='q-one'")
            .fetch_one(pool)
            .await
            .unwrap();
    assert_eq!(state, "queued", "expiry retains explicit review gate");
    control
        .task_command_submit(
            "crypto",
            access,
            request(
                "release-edit",
                w::Action::CancelEdit {
                    queue_id: "q-one".into(),
                    lease_id: editing["leaseId"].as_str().unwrap().into(),
                },
            ),
            later,
        )
        .await
        .unwrap();
    let (a, b) = tokio::join!(
        control.work_task_drain_once(later),
        control.work_task_drain_once(later)
    );
    a.unwrap();
    b.unwrap();
    let (cid,rev):(String,i64)=sqlx::query_as("SELECT claimed_command_id,revision FROM zuse_rust_task_command_queue WHERE queue_id='q-one'").fetch_one(pool).await.unwrap();
    assert!(cid.starts_with("queue:"));
    assert_eq!(rev, 1);
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM zuse_rust_task_command_execution WHERE command_id=$1",
    )
    .bind(&cid)
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(count, 1, "simultaneous workers claim exactly once");
    let page = control
        .task_history(access, "crypto", "session:crypto", None)
        .await
        .unwrap();
    let text = page["commandAdmissions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["messageId"] == "mid-one")
        .unwrap()["text"]
        .as_str()
        .unwrap()
        .to_owned();
    for value in [
        "Final edited text",
        "src/lib.rs",
        "skills/test.md",
        "Reviewed user context",
    ] {
        assert!(text.contains(value));
    }
    sqlx::query("UPDATE zuse_rust_task_command_execution SET state='running',turn_id='queue-active-turn' WHERE command_id=$1")
        .bind(&cid)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("UPDATE zuse_rust_mailbox_commands SET state='applied',ever_leased=true WHERE command_id=$1").bind(&cid).execute(pool).await.unwrap();
    control
        .task_command_submit(
            "crypto",
            access,
            request(
                "queue-steer",
                w::Action::Queue {
                    queue_id: "q-steer".into(),
                    message_id: "mid-steer".into(),
                    input: input("Use this additional context"),
                    hold_for_turn_end: false,
                },
            ),
            later,
        )
        .await
        .unwrap();
    control
        .task_command_submit(
            "crypto",
            access,
            request(
                "steer-current",
                w::Action::SteerNow {
                    queue_id: "q-steer".into(),
                    revision: 1,
                    based_on: None,
                },
            ),
            later,
        )
        .await
        .unwrap();
    let steered = super::interrupts::native(control, pool, "crypto", "steer-current").await;
    assert_eq!(
        steered.based_on.unwrap().turn_id.as_deref(),
        Some("queue-active-turn"),
        "UI omitted target must bind factual current PG turn"
    );
    control
        .task_command_submit(
            "crypto",
            access,
            request(
                "queue-stale-steer",
                w::Action::Queue {
                    queue_id: "q-stale-steer".into(),
                    message_id: "mid-stale-steer".into(),
                    input: input("Do not steer a newer turn"),
                    hold_for_turn_end: false,
                },
            ),
            later,
        )
        .await
        .unwrap();
    assert_eq!(
        control
            .task_command_submit(
                "crypto",
                access,
                request(
                    "stale-steer",
                    w::Action::SteerNow {
                        queue_id: "q-stale-steer".into(),
                        revision: 1,
                        based_on: Some(zuse_protocol::session_commands::CommandBasedOn {
                            turn_id: Some("old-turn".into()),
                            frontier: None
                        })
                    }
                ),
                later
            )
            .await
            .unwrap_err()
            .code,
        "cloud_task_steer_turn_changed"
    );
    control
        .task_command_submit(
            "crypto",
            access,
            request(
                "remove-stale-steer",
                w::Action::Delete {
                    queue_id: "q-stale-steer".into(),
                    revision: 1,
                    lease_id: None,
                },
            ),
            later,
        )
        .await
        .unwrap();
    let ordinary=serde_json::from_value(json!({"commandId":"ordinary-later","sessionId":"session:crypto","action":{"type":"submit","messageId":"ordinary-mid","payload":{"kind":"run","messageId":"ordinary-mid","request":{"prompt":"Previously accepted future ordinary message","harness":"codex","model":"fixture","cwd":".","sandbox":"workspace-write"}}}})).unwrap();
    control
        .task_command_submit("crypto", access, ordinary, later)
        .await
        .unwrap();
    control
        .task_command_submit(
            "crypto",
            access,
            request(
                "queue-two",
                w::Action::Queue {
                    queue_id: "q-two".into(),
                    message_id: "mid-two".into(),
                    input: input("Next after interrupt"),
                    hold_for_turn_end: false,
                },
            ),
            later,
        )
        .await
        .unwrap();
    let send = request(
        "send-now",
        w::Action::SendNow {
            queue_id: "q-two".into(),
            revision: 1,
        },
    );
    let first = control
        .task_command_submit("crypto", access, send.clone(), later)
        .await
        .unwrap();
    assert_eq!(
        control
            .task_command_submit("crypto", access, send, later)
            .await
            .unwrap(),
        first
    );
    assert_eq!(
        control
            .task_command_queue("crypto", access, "session:crypto")
            .await
            .unwrap()["paused"],
        false,
        "SendNow internal Interrupt must not newly pause an unpaused queue"
    );
    let (priority,control_id):(i16,String)=sqlx::query_as("SELECT priority,wait_for_control_id FROM zuse_rust_task_command_execution WHERE command_id='send-now'").fetch_one(pool).await.unwrap();
    assert_eq!(priority, 1);
    let lane: String =
        sqlx::query_scalar("SELECT lane FROM zuse_rust_mailbox_commands WHERE command_id=$1")
            .bind(&control_id)
            .fetch_one(pool)
            .await
            .unwrap();
    assert_eq!(lane, "task-control:session:crypto");
    async fn eligible(pool: &PgPool) -> Vec<String> {
        sqlx::query_scalar(&format!("SELECT c.command_id FROM zuse_rust_mailbox_commands c WHERE c.workspace_id='crypto' AND c.lane='task-execution:session:crypto' AND c.state IN ('accepted','waiting-for-runtime') {} ORDER BY workspace_sequence",crate::runtime::task_commands::dispatch::EXECUTION_GUARD)).fetch_all(pool).await.unwrap()
    }
    assert!(
        !eligible(pool).await.iter().any(|v| v == "send-now"),
        "Run waits exact Interrupt admission"
    );
    sqlx::query("UPDATE zuse_rust_mailbox_commands SET state='applied' WHERE command_id=$1")
        .bind(&control_id)
        .execute(pool)
        .await
        .unwrap();
    assert!(
        eligible(pool).await.is_empty(),
        "ever-leased current Run must terminalize factually before priority may start"
    );
    sqlx::query(
        "UPDATE zuse_rust_task_command_execution SET state='cancelled' WHERE command_id=$1",
    )
    .bind(&cid)
    .execute(pool)
    .await
    .unwrap();
    assert_eq!(
        eligible(pool).await,
        vec!["send-now".to_owned()],
        "unleased ordinary Run yields to SendNow priority after actual prior terminal"
    );
    control
        .task_command_submit(
            "crypto",
            access,
            request(
                "queue-negative",
                w::Action::Queue {
                    queue_id: "q-negative".into(),
                    message_id: "mid-negative".into(),
                    input: input("Must not run after rejected Interrupt"),
                    hold_for_turn_end: false,
                },
            ),
            later,
        )
        .await
        .unwrap();
    control
        .task_command_submit(
            "crypto",
            access,
            request(
                "negative-send",
                w::Action::SendNow {
                    queue_id: "q-negative".into(),
                    revision: 1,
                },
            ),
            later,
        )
        .await
        .unwrap();
    let (negative_control,fingerprint):(String,String)=sqlx::query_as("SELECT c.command_id,c.fingerprint FROM zuse_rust_task_command_execution e JOIN zuse_rust_mailbox_commands c ON c.workspace_id=e.workspace_id AND c.command_id=e.wait_for_control_id WHERE e.command_id='negative-send'").fetch_one(pool).await.unwrap();
    let mut negative_tx = pool.begin().await.unwrap();
    control
        .task_delivery_fact_tx(
            &mut negative_tx,
            "crypto",
            &zuse_protocol::cloud_commands::RuntimeAcknowledgment {
                command_id: negative_control,
                lease_token: "fixture-verified-upstream".into(),
                fingerprint,
                state: zuse_protocol::cloud_commands::AcknowledgmentState::Rejected,
                category: None,
                result_iv: None,
                result_ciphertext: None,
            },
            later,
        )
        .await
        .unwrap();
    negative_tx.commit().await.unwrap();
    let (negative_state,ever_leased):(String,bool)=sqlx::query_as("SELECT e.state,c.ever_leased FROM zuse_rust_task_command_execution e JOIN zuse_rust_mailbox_commands c USING(workspace_id,command_id) WHERE e.command_id='negative-send'").fetch_one(pool).await.unwrap();
    assert_eq!(negative_state, "cancelled");
    assert!(
        !ever_leased,
        "only a proven undispatched dependent may be cancelled"
    );
    let mut goal = input("Actual goal requires goal executor");
    goal.as_goal = true;
    control
        .task_command_submit(
            "crypto",
            access,
            request(
                "queue-goal",
                w::Action::Queue {
                    queue_id: "q-goal".into(),
                    message_id: "mid-goal".into(),
                    input: goal,
                    hold_for_turn_end: true,
                },
            ),
            later,
        )
        .await
        .unwrap();
    sqlx::query("UPDATE zuse_rust_task_command_execution SET state='completed'")
        .execute(pool)
        .await
        .unwrap();
    control.work_task_drain_once(later).await.unwrap();
    let (state,reason):(String,String)=sqlx::query_as("SELECT q.state,d.last_error FROM zuse_rust_task_command_queue q JOIN zuse_rust_task_command_drains d USING(workspace_id,session_id) WHERE queue_id='q-goal'").fetch_one(pool).await.unwrap();
    assert_eq!(state, "queued");
    assert_eq!(reason, "cloud_task_queue_goal_execution_unavailable");
    // The production endpoint dispatcher promotes after commit without waiting
    // for a provider worker round. The same durable job remains the fallback.
    let live_now = chrono::Utc::now().timestamp_millis();
    control
        .task_command_submit(
            "crypto",
            access,
            request(
                "review-goal-as-normal",
                w::Action::Edit {
                    queue_id: "q-goal".into(),
                    revision: 1,
                    input: input("Reviewed as an ordinary message"),
                    lease_id: None,
                },
            ),
            live_now,
        )
        .await
        .unwrap();
    control.dispatch_task_drain("crypto", Some("session:crypto"));
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            let claimed: bool = sqlx::query_scalar(
                "SELECT state='claimed' FROM zuse_rust_task_command_queue WHERE queue_id='q-goal'",
            )
            .fetch_one(pool)
            .await
            .unwrap();
            if claimed {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("direct dispatcher must not wait for a provider worker");
}
