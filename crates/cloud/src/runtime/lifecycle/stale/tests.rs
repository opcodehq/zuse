//! REGRESSION GUARD for "a paused computer can never be archived or used".
//!
//! Invariant: an unfinished lifecycle operation on a paused computer, with
//! no progress for its deadline and no active job, is settled by a pause that
//! sleeps the machine, after which Archive (and Resume, and a message's wake)
//! is admitted. Operations another path still owns are left alone.
use super::*;
use crate::{access::Scope, auth::Principal};
use std::sync::{
    Mutex,
    atomic::{AtomicUsize, Ordering},
};

struct Provider {
    state: Mutex<MachineState>,
    sleeps: AtomicUsize,
}
#[async_trait]
impl RetainedProvider for Provider {
    async fn inspect_retained(&self, id: &str) -> Result<Option<Machine>, ProviderError> {
        assert_eq!(id, "machine-bob");
        Ok(Some(Machine {
            id: id.into(),
            name: "machine-bob".into(),
            state: self.state.lock().unwrap().clone(),
            vcpu: None,
            memory_bytes: None,
        }))
    }
    async fn pause_retained(&self, id: &str) -> Result<(), ProviderError> {
        assert_eq!(id, "machine-bob");
        *self.state.lock().unwrap() = MachineState::Stopped;
        self.sleeps.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    async fn wake_retained(&self, _: &str, _: &MachineState) -> Result<(), ProviderError> {
        panic!("settling never wakes the computer")
    }
    async fn launch_retained(&self, _: &Directive, _: &str, _: &str) -> Result<(), ProviderError> {
        panic!("settling never launches a runtime")
    }
}

fn bob() -> Access {
    Access {
        actor: Principal {
            account_id: "user-bob".into(),
        },
        scope: Scope::Personal,
        owner_id: "user-bob".into(),
        membership: None,
    }
}

struct Leftover {
    action: &'static str,
    phase: &'static str,
    /// Lifecycle job of the operation: (state, last error code).
    job: (&'static str, &'static str),
    status: &'static str,
    /// Merged into the fixture's request_config.
    patch: Value,
    /// Milliseconds since the operation last progressed.
    age: i64,
}

/// What an older server left behind after giving up on a resume: the
/// computer marked paused, the operation itself still `launching`, its job
/// failed after its last attempt.
fn incident() -> Leftover {
    Leftover {
        action: "resume",
        phase: "launching",
        job: ("failed", "cloud_lifecycle_timed_out"),
        status: "cloud_lifecycle_timed_out",
        patch: json!({}),
        age: DEADLINE_MS + 1,
    }
}

async fn leave(pool: &PgPool, config: &Value, now: i64, left: &Leftover) -> String {
    let operation = uuid::Uuid::new_v4().to_string();
    let mut config = config.clone();
    config["rustLifecycleOperation"] =
        json!({"id":operation,"action":left.action,"phase":"paused"});
    for (key, value) in left.patch.as_object().unwrap() {
        config[key] = value.clone();
    }
    sqlx::query("UPDATE api_cloud_workspaces SET state='paused',desired_state='paused',runtime_state='offline',provider_sandbox_id='machine-bob',status_code=$1,request_config=$2 WHERE workspace_id='ws-bob'")
        .bind(left.status).bind(&config).execute(pool).await.unwrap();
    let at = now - left.age;
    sqlx::query("INSERT INTO zuse_rust_lifecycle_operations(operation_id,workspace_id,command_id,generation,source_machine_id,storage_incarnation_id,action,phase,created_at,updated_at) VALUES($1,'ws-bob',$2,1,'machine-bob','disk',$3,$4,$5,$5)")
        .bind(&operation).bind(format!("left:{operation}")).bind(left.action).bind(left.phase).bind(at)
        .execute(pool).await.unwrap();
    let mut tx = pool.begin().await.unwrap();
    let job = jobs::enqueue(
        &mut tx,
        &format!("lifecycle:{operation}"),
        JOB_KIND,
        "ws-bob",
        1,
        &json!({"operationId":operation}),
        at,
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    sqlx::query("UPDATE zuse_rust_jobs SET state=$2,last_error_code=NULLIF($3,''),attempts=120,max_attempts=120 WHERE job_id=$1")
        .bind(job).bind(left.job.0).bind(left.job.1).execute(pool).await.unwrap();
    operation
}

async fn row(pool: &PgPool) -> Value {
    sqlx::query_scalar("SELECT to_jsonb(w) FROM api_cloud_workspaces w WHERE workspace_id='ws-bob'")
        .fetch_one(pool)
        .await
        .unwrap()
}

/// Run the lifecycle worker until no job is due.
async fn drain(c: &RuntimeControl, pool: &PgPool, provider: &Provider) {
    for _ in 0..10 {
        sqlx::query("UPDATE zuse_rust_jobs SET due_at=0 WHERE state='pending'")
            .execute(pool)
            .await
            .unwrap();
        if !c.lifecycle_once(provider, clock_ms()).await.unwrap() {
            return;
        }
    }
    panic!("lifecycle work did not finish");
}

async fn archive(c: &RuntimeControl, now: i64) -> Result<Value, ApiError> {
    c.lifecycle_action(
        "ws-bob",
        &bob(),
        Action::Archive,
        Some(&uuid::Uuid::new_v4().to_string()),
        now,
    )
    .await
}

async fn drop_schema(admin: PgPool, pool: PgPool, schema: String) {
    pool.close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "requires isolated localhost PostgreSQL"]
async fn a_resume_an_older_server_gave_up_on_is_settled_slept_and_archivable() {
    let (admin, pool, schema, c, now, config) = crate::runtime::recreation::tests::fixture().await;
    sqlx::query("INSERT INTO api_cloud_billing_periods VALUES('stale-period','user-bob','polar',NULL,'manual','USD',$1,$2,1000,1000,500,500,$1)").bind(now-1000).bind(now+86400000).execute(&pool).await.unwrap();
    let resume = leave(&pool, &config, now, &incident()).await;
    // The incident: neither Archive nor a message's wake is admitted.
    assert_eq!(
        archive(&c, now).await.unwrap_err().code,
        "cloud_lifecycle_quiescence_required"
    );
    // The machine booted; only the runtime never started.
    let provider = Provider {
        state: Mutex::new(MachineState::Running),
        sleeps: AtomicUsize::new(0),
    };
    assert_eq!(c.settle_stale_lifecycle(now).await.unwrap(), 1);
    // Settled once: the pause it queued now owns the computer.
    assert_eq!(c.settle_stale_lifecycle(now).await.unwrap(), 0);
    let settled = row(&pool).await;
    assert_eq!(settled["state"], "pausing");
    assert_eq!(settled["status_code"], "cloud_lifecycle_timed_out");
    let config = &settled["request_config"];
    assert_eq!(config["rustLifecycleOperation"]["action"], "pause");
    assert_eq!(config["rustResumeFailure"]["operationId"], json!(resume));
    assert_eq!(
        config["rustResumeFailure"]["pauseOperationId"],
        config["rustLifecycleOperation"]["id"]
    );
    // The pause sleeps the machine through the ordinary lifecycle and keeps
    // the failure visible.
    drain(&c, &pool, &provider).await;
    assert_eq!(provider.sleeps.load(Ordering::SeqCst), 1);
    let paused = row(&pool).await;
    assert_eq!(
        (&paused["state"], &paused["status_code"]),
        (&json!("paused"), &json!("cloud_lifecycle_timed_out"))
    );
    let phase: String = sqlx::query_scalar(
        "SELECT phase FROM zuse_rust_lifecycle_operations WHERE operation_id=$1",
    )
    .bind(paused["request_config"]["rustLifecycleOperation"]["id"].as_str())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(phase, "paused");
    assert_eq!(c.settle_stale_lifecycle(clock_ms()).await.unwrap(), 0);
    // Recreate is still offered for the failed resume.
    assert!(c.recreation_eligible(&paused));
    // And the computer can be archived.
    let archived = archive(&c, now).await.unwrap();
    assert_eq!(archived["state"], "archived");
    drop_schema(admin, pool, schema).await;
}

#[tokio::test]
#[ignore = "requires isolated localhost PostgreSQL"]
async fn an_unfinished_pause_on_a_paused_computer_is_settled_without_a_resume_failure() {
    let (admin, pool, schema, c, now, config) = crate::runtime::recreation::tests::fixture().await;
    leave(
        &pool,
        &config,
        now,
        &Leftover {
            action: "pause",
            phase: "pausing",
            job: ("failed", "job_attempts_exhausted"),
            status: "paused",
            ..incident()
        },
    )
    .await;
    // Already asleep at the provider: confirmed, not slept again.
    let provider = Provider {
        state: Mutex::new(MachineState::Stopped),
        sleeps: AtomicUsize::new(0),
    };
    assert_eq!(c.settle_stale_lifecycle(now).await.unwrap(), 1);
    let settled = row(&pool).await;
    // No resume failure code to keep: the generic one shows while it sleeps.
    assert_eq!(settled["status_code"], CODE);
    assert!(settled["request_config"]["rustResumeFailure"].is_null());
    drain(&c, &pool, &provider).await;
    assert_eq!(provider.sleeps.load(Ordering::SeqCst), 0);
    let paused = row(&pool).await;
    assert_eq!(
        (&paused["state"], &paused["status_code"]),
        (&json!("paused"), &json!("paused"))
    );
    assert_eq!(archive(&c, now).await.unwrap()["state"], "archived");
    drop_schema(admin, pool, schema).await;
}

#[tokio::test]
#[ignore = "requires isolated localhost PostgreSQL"]
async fn operations_another_path_still_owns_are_left_alone() {
    let cases: Vec<(&str, Leftover, Option<&str>)> = vec![
        (
            "within its deadline",
            Leftover {
                age: DEADLINE_MS - 60_000,
                ..incident()
            },
            None,
        ),
        (
            "its job is still retrying",
            Leftover {
                job: ("pending", ""),
                ..incident()
            },
            None,
        ),
        (
            "another job is active on the computer",
            incident(),
            Some("workspace-restore"),
        ),
        (
            "a recreation owns the wake",
            Leftover {
                patch: json!({"rustRecreation":{"operationId":"recreation","phase":"waking"}}),
                ..incident()
            },
            None,
        ),
        (
            "a repair owns the computer",
            Leftover {
                patch: json!({"rustRuntimeRepair":{"operationId":"repair","phase":"quiescing"}}),
                ..incident()
            },
            None,
        ),
        (
            "the provider no longer has the machine (Recreate's case)",
            Leftover {
                job: ("failed", MACHINE_MISSING),
                status: MACHINE_MISSING,
                ..incident()
            },
            None,
        ),
        (
            "the computer is being deleted",
            Leftover {
                patch: json!({"cloudMailboxLifecyclePending":{"action":"delete"}}),
                ..incident()
            },
            None,
        ),
        (
            "the operation finished",
            Leftover {
                phase: "ready",
                ..incident()
            },
            None,
        ),
    ];
    for (name, left, other_job) in cases {
        let (admin, pool, schema, c, now, config) =
            crate::runtime::recreation::tests::fixture().await;
        let operation = leave(&pool, &config, now, &left).await;
        if let Some(kind) = other_job {
            let mut tx = pool.begin().await.unwrap();
            jobs::enqueue(
                &mut tx,
                &format!("other:{operation}"),
                kind,
                "ws-bob",
                1,
                &json!({"operationId":"other"}),
                now,
            )
            .await
            .unwrap();
            tx.commit().await.unwrap();
        }
        let before = row(&pool).await;
        assert_eq!(
            c.settle_stale_lifecycle(now).await.unwrap(),
            0,
            "{name}: settled"
        );
        assert_eq!(row(&pool).await, before, "{name}: changed");
        drop_schema(admin, pool, schema).await;
    }
    // Memory recovery owns its operation.
    let (admin, pool, schema, c, now, config) = crate::runtime::recreation::tests::fixture().await;
    let operation = leave(&pool, &config, now, &incident()).await;
    sqlx::query("UPDATE api_cloud_workspaces SET request_config=jsonb_set(request_config,'{rustMemoryTransportRecovery}',jsonb_build_object('operationId',$1::text)) WHERE workspace_id='ws-bob'")
        .bind(&operation).execute(&pool).await.unwrap();
    assert_eq!(c.settle_stale_lifecycle(now).await.unwrap(), 0);
    drop_schema(admin, pool, schema).await;
}
