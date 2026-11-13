//! Triggers and retirement: the hourly staleness pass rebuilds active
//! accounts whose image inputs changed and nothing else; inactive accounts
//! rebuild on use; failures back off and stop at a cap; old images retire
//! only when nothing references them and nothing could select them.
use super::schedule::*;
use super::tests::FakeHost;
use super::worker_tests::{Database, control, run, save};
use super::*;
use serde_json::json;
use sqlx::PgPool;

const DAY: i64 = 24 * 3600 * 1000;

async fn workspace(pool: &PgPool, id: &str, owner: &str, active: i64, config: Value) {
    sqlx::query("INSERT INTO api_cloud_workspaces(workspace_id,account_id,project_id,build_id,provider,runtime_state,chat_id,initial_session_id,branch,base_ref,state,desired_state,status_code,request_config,revision,created_at,updated_at,last_activity_at) VALUES($1,$2,'project','build','boxd','offline','chat','session',$1,'main','ready','ready','ready',$3,0,0,0,$4)")
        .bind(id).bind(owner).bind(config).bind(active).execute(pool).await.unwrap();
}

async fn rows(pool: &PgPool, owner: &str) -> Vec<Value> {
    sqlx::query_scalar("SELECT to_jsonb(i) FROM cloud_machine_images i WHERE owner_id=$1 ORDER BY created_at,image_id")
        .bind(owner)
        .fetch_all(pool)
        .await
        .unwrap()
}

fn reset_guest(host: &FakeHost, exit: i64) {
    let mut state = host.0.lock().unwrap();
    state.setup_launches = 0;
    state.setup_polls = 0;
    state.guest.setup_exit = exit;
}

#[tokio::test]
#[ignore = "requires explicit isolated localhost PostgreSQL"]
async fn hourly_staleness_rebuilds_changed_inputs_only_and_retires_unused_images() {
    let db = Database::new("uimgs").await;
    let pool = db.pool.clone();
    // Clocks in this test are real-sized so the activity window means something.
    let start = 400 * DAY;
    sqlx::query("DELETE FROM api_cloud_workspaces")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO api_cloud_billing_periods VALUES('period-bob','user-bob','manual','USD',0,9999999999999,0,0,0)").execute(&pool).await.unwrap();
    db.base("base-v1", "base-snapshot", 1).await;
    let host = FakeHost::new();
    let control = control(&pool, &host);
    for owner in ["user-alice", "user-bob"] {
        save(&pool, owner, 0, "npm i -g pnpm", start).await;
    }
    let mut now = run(&control, start + worker::SAVE_DEBOUNCE_MS, 80).await;
    for owner in ["user-alice", "user-bob"] {
        assert_eq!(rows(&pool, owner).await[0]["state"], "ready");
    }
    let first_alice = rows(&pool, "user-alice").await[0].clone();
    // Alice used a computer last week (on her image); Bob has not for months.
    workspace(
        &pool,
        "alice-task",
        "user-alice",
        now - 7 * DAY,
        json!({"rustMachineImage":{"imageId":first_alice["image_id"]}}),
    )
    .await;
    workspace(&pool, "bob-task", "user-bob", now - 90 * DAY, json!({})).await;

    // Nothing changed: a quiet account builds nothing.
    assert_eq!(control.staleness_pass(now).await.unwrap(), 0);

    // A new managed base (its snapshot exists at the provider too).
    {
        let mut state = host.0.lock().unwrap();
        let mut snapshot = state.snapshots["base-snapshot"].clone();
        snapshot.0.id = "base-snapshot-v2".into();
        snapshot.0.name = "base-snapshot-v2".into();
        state.snapshots.insert("base-snapshot-v2".into(), snapshot);
    }
    reset_guest(&host, 0);
    db.base("base-v2", "base-snapshot-v2", 2).await;
    // Only active Alice is rebuilt in the background, once.
    assert_eq!(control.staleness_pass(now).await.unwrap(), 1);
    assert_eq!(
        control.staleness_pass(now).await.unwrap(),
        0,
        "already queued"
    );
    let alice = rows(&pool, "user-alice").await;
    assert_eq!(
        (alice[1]["state"].as_str(), alice[1]["reason"].as_str()),
        (Some("queued"), Some("base"))
    );
    assert_eq!(rows(&pool, "user-bob").await.len(), 1);
    now = run(&control, now, 40).await;
    let alice = rows(&pool, "user-alice").await;
    assert_eq!(alice[1]["state"], "ready", "{alice:?}");
    assert_eq!(alice[1]["base_release_id"], "base-v2");
    assert_eq!(
        control.staleness_pass(now).await.unwrap(),
        0,
        "inputs now match"
    );
    // Alice's first image is still referenced by her computer: kept.
    assert_eq!(alice[0]["state"], "ready");
    assert!(!control.retire_one(now).await.unwrap());
    // Once that computer is deleted, the replaced image retires and its
    // provider snapshot is deleted. The newest image stays.
    sqlx::query("UPDATE api_cloud_workspaces SET state='deleted' WHERE workspace_id='alice-task'")
        .execute(&pool)
        .await
        .unwrap();
    now = run(&control, now, 6).await;
    let alice = rows(&pool, "user-alice").await;
    assert_eq!(alice[0]["state"], "retired", "{alice:?}");
    assert!(alice[0]["retired_at"].as_i64().is_some());
    assert_eq!(alice[1]["state"], "ready");
    {
        let state = host.0.lock().unwrap();
        assert!(
            !state
                .snapshots
                .contains_key(first_alice["artifact_id"].as_str().unwrap())
        );
        assert!(
            state
                .snapshots
                .contains_key(alice[1]["artifact_id"].as_str().unwrap())
        );
    }

    // Bob starts a computer: his rebuild is queued on use, once.
    let mut tx = pool.begin().await.unwrap();
    let digest = selection::current_digest_tx(&mut tx, "user-bob")
        .await
        .unwrap()
        .unwrap();
    assert!(
        rebuild_on_use_tx(&mut tx, "user-bob", "boxd", &digest, now)
            .await
            .unwrap()
    );
    assert!(
        !rebuild_on_use_tx(&mut tx, "user-bob", "boxd", &digest, now)
            .await
            .unwrap()
    );
    tx.commit().await.unwrap();
    let bob = rows(&pool, "user-bob").await;
    assert_eq!(
        (bob[1]["state"].as_str(), bob[1]["reason"].as_str()),
        (Some("queued"), Some("use"))
    );
    sqlx::query("UPDATE cloud_machine_images SET state='superseded' WHERE owner_id='user-bob' AND state='queued'").execute(&pool).await.unwrap();

    // A broken script edit: the build fails, the previous image keeps serving,
    // and later passes retry with backoff, at most MAX_FAILED_ATTEMPTS times.
    sqlx::query("UPDATE api_cloud_workspaces SET state='ready',last_activity_at=$1 WHERE workspace_id='alice-task'").bind(now).execute(&pool).await.unwrap();
    reset_guest(&host, 1);
    save(&pool, "user-alice", 1, "apt-get install -y nope", now).await;
    now = run(&control, now + worker::SAVE_DEBOUNCE_MS, 30).await;
    let failed = |rows: &[Value]| rows.iter().filter(|r| r["state"] == "failed").count();
    assert_eq!(failed(&rows(&pool, "user-alice").await), 1);
    assert_eq!(control.staleness_pass(now).await.unwrap(), 0, "backing off");
    let mut attempts = 1;
    for hours in [1, 3, 7, 15] {
        let at = now + hours * SCHEDULE_INTERVAL_MS;
        let queued = control.staleness_pass(at).await.unwrap();
        if queued == 1 {
            attempts += 1;
            reset_guest(&host, 1);
            run(&control, at, 30).await;
        }
    }
    assert_eq!(attempts as i64, MAX_FAILED_ATTEMPTS);
    let alice = rows(&pool, "user-alice").await;
    assert_eq!(failed(&alice) as i64, MAX_FAILED_ATTEMPTS);
    assert!(
        alice
            .iter()
            .any(|r| r["state"] == "ready" && r["base_release_id"] == "base-v2")
    );
    assert_eq!(
        control
            .staleness_pass(now + 100 * SCHEDULE_INTERVAL_MS)
            .await
            .unwrap(),
        0,
        "capped"
    );

    // Clearing the setup script: waiting builds are superseded and the
    // unreferenced image retires; computers use the managed base.
    sqlx::query("UPDATE api_cloud_workspaces SET state='deleted'")
        .execute(&pool)
        .await
        .unwrap();
    crate::computers::ComputerControl::from_pool(pool.clone(), vec!["user-alice".into()])
        .save_machine_as(
            "user-alice",
            "user-alice",
            None,
            zuse_protocol::cloud_computers::SaveMachine {
                expected_revision: 2,
                recipe: MachineRecipe::default(),
            },
            now + 200 * SCHEDULE_INTERVAL_MS,
        )
        .await
        .unwrap();
    run(&control, now + 201 * SCHEDULE_INTERVAL_MS, 6).await;
    let alice = rows(&pool, "user-alice").await;
    assert!(
        alice.iter().all(|r| matches!(
            r["state"].as_str(),
            Some("retired" | "superseded" | "failed")
        )),
        "{alice:?}"
    );
    db.drop().await;
}
