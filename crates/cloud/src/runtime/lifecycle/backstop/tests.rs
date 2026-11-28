//! REGRESSION GUARD for "a computer stays awake forever".
//!
//! Invariant: once nothing has happened on an awake computer (no user
//! activity, command write, lifecycle admission or launch request) for
//! `max_quiet_awake_ms`, the idle worker admits it to sleep — through the
//! ordinary idle path or the backstop — and its deadline is known so the
//! worker wakes for it. The only exceptions are
//!   * a turn that is genuinely running: state ready, runtime online, a
//!     runtime credential the runtime renewed within its lifetime, and a
//!     starting/running/waiting-input command of the current generation —
//!     never slept, however long it runs;
//!   * operations that own the machine identity (recreation, repair, memory
//!     recovery) or a computer with no retained disk — reported once to
//!     operators (`held`), never raced.
//! Nothing sleeps while there is recent activity.
//!
//! Every row below is a "stuck" combination found in the code. Changing the
//! idle SQL, awake.sql or the lifecycle eligibility must keep this green; add
//! a row for any new condition that can hold a computer awake.
use super::*;
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};

const T0: i64 = 1_000_000;
const OP: &str = "op-stuck";

fn config() -> Value {
    json!({
        "runtimeGeneration":2,"gatewayEpoch":2,"runtimeRpcProtocol":"zuse-rpc-v1",
        "commandDeliveryMode":zuse_protocol::cloud_task_commands::MODE,
        "rustCommandAdmissionVersion":2,"rustCommandMailboxEnabled":true,
        "runtimeCredentialExpiresAtMs":9_000_000_000_000_i64,
        "runtimeBootstrapReceipt":{"workspaceId":"ws-bob","generation":2,"gatewayEpoch":2},
        "runtimeCredentialKeyThumbprint":"credential-key","runtimeSigningKeyThumbprint":"signing-key",
        "rustRuntimeStorage":{"storageIncarnationId":"retained-disk"},
        "rustStorageEnrollment":{"phase":"committed","generation":2,"sourceSandboxId":"machine-bob",
            "storageIncarnationId":"retained-disk","credentialKeyThumbprint":"credential-key","signingKeyThumbprint":"signing-key",
            "capabilities":[CAPABILITY,"rust-session-commands-v1",zuse_protocol::cloud_task_commands::CAPABILITY]}
    })
}

fn control(pool: PgPool) -> RuntimeControl {
    let signing = ed25519_dalek::SigningKey::from_bytes(&[71; 32]);
    let jwk = json!({"kty":"OKP","crv":"Ed25519","d":URL_SAFE_NO_PAD.encode([71;32]),"x":URL_SAFE_NO_PAD.encode(signing.verifying_key().as_bytes())});
    let control = RuntimeControl::from_pool(
        pool,
        MasterKey::parse(&URL_SAFE_NO_PAD.encode([72; 32])).unwrap(),
        TicketSigner::from_jwk(&jwk.to_string(), "https://api.example.test").unwrap(),
    )
    .with_creation_accounts(vec!["user-bob".into()]);
    control.task_command_schema_ready.set(true).unwrap();
    control
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Expect {
    /// The ordinary idle path sleeps it one idle window after T0.
    Normal,
    /// Only the backstop sleeps it, after the max quiet bound.
    Backstop,
    /// A turn whose runtime is gone: the backstop marks it lost (for review)
    /// and sleeps the computer soon after the runtime was last alive.
    Lost,
    /// A genuinely running turn: never slept.
    Never,
    /// Reported to operators with this hold, never raced.
    Held(&'static str),
}
use Expect::*;

struct Case {
    name: &'static str,
    state: &'static str,
    desired: &'static str,
    runtime: &'static str,
    provider: &'static str,
    status: &'static str,
    /// Merged into config(); null removes the key.
    patch: Value,
    /// A command: (state, runtime generation).
    command: Option<(&'static str, i64)>,
    intent: bool,
    /// A stuck lifecycle operation (action, phase) with a pending job.
    op: Option<(&'static str, &'static str)>,
    expect: Expect,
}

fn case(name: &'static str, expect: Expect) -> Case {
    Case {
        name,
        state: "ready",
        desired: "ready",
        runtime: "online",
        provider: "boxd",
        status: "ready",
        patch: json!({}),
        command: None,
        intent: false,
        op: None,
        expect,
    }
}

fn matrix() -> Vec<Case> {
    let mut rows = vec![
        case("plain idle on boxd", Normal),
        Case {
            provider: "prized",
            ..case("plain idle on prized", Normal)
        },
        Case {
            provider: "boat",
            ..case("plain idle on boat", Normal)
        },
        Case {
            command: Some(("completed", 2)),
            ..case("completed turn", Normal)
        },
        Case {
            command: Some(("outcome-unknown", 1)),
            ..case("outcome-unknown turn from a replaced runtime", Normal)
        },
        Case {
            command: Some(("outcome-unknown", 2)),
            ..case("outcome-unknown turn", Normal)
        },
        Case {
            command: Some(("queued", 2)),
            ..case("queued command never delivered", Backstop)
        },
        Case {
            runtime: "offline",
            command: Some(("queued", 2)),
            ..case("queued command, runtime offline", Backstop)
        },
        Case {
            intent: true,
            ..case("launch intent never consumed", Backstop)
        },
        Case {
            patch: json!({"runtimeReleaseChangeRequested":true}),
            ..case("runtime release change requested", Backstop)
        },
        Case {
            runtime: "offline",
            ..case("runtime offline", Backstop)
        },
        Case {
            patch: json!({"runtimeCredentialExpiresAtMs":0}),
            ..case("runtime credential expired or mismatched", Normal)
        },
        Case {
            patch: json!({"runtimeBootstrapReceipt":null}),
            ..case("missing bootstrap receipt", Backstop)
        },
        Case {
            patch: json!({"rustStorageEnrollment":null}),
            ..case("missing storage enrollment", Backstop)
        },
        Case {
            patch: json!({"rustCommandMailboxEnabled":false}),
            ..case("command mailbox disabled", Backstop)
        },
        Case {
            state: "setup",
            status: "repository_preparation_failed",
            ..case("setup failed preparing the repository", Backstop)
        },
        Case {
            state: "setup",
            status: "repository-syncing",
            command: Some(("running", 2)),
            ..case("early launch turn during setup, runtime alive", Never)
        },
        Case {
            state: "provisioning",
            runtime: "offline",
            status: "starting-runtime",
            ..case("setup never finished", Backstop)
        },
        Case {
            patch: json!({"rustRecreation":{"operationId":"rec","phase":"capturing"}}),
            ..case("recreation in progress", Held("recreation"))
        },
        Case {
            patch: json!({"rustRecreation":{"operationId":"rec","phase":"attention-required"}}),
            ..case("recreation needs attention", Held("recreation"))
        },
        Case {
            patch: json!({"rustRecreation":{"operationId":"rec","phase":"ready"}}),
            ..case("recreation finished", Normal)
        },
        // The ordinary path does not consult the repair marker (unchanged
        // behavior); the backstop alone treats a repair as owning the machine.
        Case {
            patch: json!({"rustRuntimeRepair":{"phase":"attention-required"}}),
            ..case("runtime repair needs attention", Normal)
        },
        Case {
            patch: json!({"rustRuntimeRepair":{"phase":"stopping"},"runtimeReleaseChangeRequested":true}),
            ..case(
                "runtime repair in progress, otherwise ineligible",
                Held("repair"),
            )
        },
        Case {
            patch: json!({"rustRuntimeStorage":null}),
            ..case("no retained storage", Held("no-retained-storage"))
        },
        Case {
            state: "provisioning",
            runtime: "offline",
            status: "cloud_retained_provider_failed",
            op: Some(("resume", "launching")),
            patch: json!({"rustMemoryTransportRecovery":{"operationId":OP}}),
            ..case("memory transport recovery", Held("memory-recovery"))
        },
    ];
    for phase in ["preparing", "launching", "confirming", "rolling-back"] {
        rows.push(Case {
            patch: json!({"runtimeActivation":{"phase":phase}}),
            ..case(
                Box::leak(format!("runtime activation {phase}").into_boxed_str()),
                Backstop,
            )
        });
    }
    for state in ["starting", "running", "waiting-input"] {
        let name = |what: &str| -> &'static str {
            Box::leak(format!("{state} turn, {what}").into_boxed_str())
        };
        rows.push(Case {
            command: Some((state, 2)),
            ..case(name("runtime alive"), Never)
        });
        rows.push(Case {
            command: Some((state, 2)),
            patch: json!({"runtimeCredentialExpiresAtMs":0}),
            ..case(name("runtime credential expired"), Lost)
        });
        rows.push(Case {
            command: Some((state, 2)),
            runtime: "offline",
            ..case(name("runtime offline"), Backstop)
        });
        rows.push(Case {
            command: Some((state, 1)),
            ..case(name("bound to a replaced runtime"), Backstop)
        });
    }
    for (action, phase, state) in [
        ("pause", "quiescing", "pausing"),
        ("pause", "quiesced", "pausing"),
        ("pause", "pausing", "pausing"),
        ("resume", "waking", "provisioning"),
        ("resume", "launching", "provisioning"),
    ] {
        rows.push(Case {
            state,
            desired: if action == "resume" {
                "ready"
            } else {
                "paused"
            },
            runtime: "offline",
            status: "cloud_retained_provider_failed",
            op: Some((action, phase)),
            ..case(
                Box::leak(format!("{action} stuck in {phase}").into_boxed_str()),
                Backstop,
            )
        });
    }
    rows
}

async fn reset(pool: &PgPool, row: &Case) {
    sqlx::raw_sql("DELETE FROM zuse_rust_task_command_execution; DELETE FROM api_cloud_workspace_launch_intents; DELETE FROM zuse_rust_lifecycle_operations; DELETE FROM zuse_rust_jobs; DELETE FROM zuse_rust_account_event_outbox;")
        .execute(pool).await.unwrap();
    let mut config = config();
    for (key, value) in row.patch.as_object().unwrap() {
        if value.is_null() {
            config.as_object_mut().unwrap().remove(key);
        } else {
            config[key] = value.clone();
        }
    }
    if let Some((action, phase)) = row.op {
        config["rustLifecycleOperation"] = json!({"id":OP,"action":action,"phase":phase});
        sqlx::query("INSERT INTO zuse_rust_lifecycle_operations(operation_id,workspace_id,command_id,generation,source_machine_id,storage_incarnation_id,action,phase,created_at,updated_at) VALUES($1,'ws-bob','stuck',2,'machine-bob','retained-disk',$2,$3,$4,$4)")
            .bind(OP).bind(action).bind(phase).bind(T0).execute(pool).await.unwrap();
        sqlx::query("INSERT INTO zuse_rust_jobs(job_id,idempotency_key,kind,resource_id,resource_generation,payload,due_at,created_at,updated_at,attempts,max_attempts,state,last_error_code) VALUES('job-stuck','lifecycle:stuck',$1,'ws-bob',2,$2,$3,$3,$3,120,120,'failed','cloud_retained_provider_failed')")
            .bind(JOB_KIND).bind(json!({"operationId":OP})).bind(T0).execute(pool).await.unwrap();
    }
    sqlx::query("UPDATE api_cloud_workspaces SET state=$1,desired_state=$2,runtime_state=$3,provider=$4,status_code=$5,request_config=$6,provider_sandbox_id='machine-bob',last_activity_at=$7,next_action_at=0 WHERE workspace_id='ws-bob'")
        .bind(row.state).bind(row.desired).bind(row.runtime).bind(row.provider).bind(row.status).bind(&config).bind(T0)
        .execute(pool).await.unwrap();
    if let Some((state, generation)) = row.command {
        sqlx::query(
            "INSERT INTO zuse_rust_task_command_execution VALUES('ws-bob','command',$1,$2,$3)",
        )
        .bind(state)
        .bind(T0)
        .bind(generation)
        .execute(pool)
        .await
        .unwrap();
    }
    if row.intent {
        sqlx::query("INSERT INTO api_cloud_workspace_launch_intents VALUES('ws-bob','user-bob','chat-bob','session-bob','turn','launch','synthetic',999999999,$1)")
            .bind(T0).execute(pool).await.unwrap();
    }
}

/// The workspace, if a new sleep (any pause other than the fixture's own
/// stuck operation) was admitted.
async fn slept(pool: &PgPool) -> Option<Value> {
    sqlx::query_scalar("SELECT to_jsonb(w) FROM api_cloud_workspaces w WHERE workspace_id='ws-bob' AND state='pausing' AND desired_state='paused' AND EXISTS(SELECT 1 FROM zuse_rust_lifecycle_operations o WHERE o.workspace_id=w.workspace_id AND o.operation_id<>$1 AND o.action='pause')")
        .bind(OP).fetch_optional(pool).await.unwrap()
}

#[tokio::test]
#[ignore = "requires explicit isolated localhost PostgreSQL"]
async fn regression_guard_no_computer_stays_awake_past_max_quiet() {
    let url = std::env::var("ZUSE_CLOUD_TEST_DATABASE_URL").unwrap();
    assert!(matches!(
        reqwest::Url::parse(&url).unwrap().host_str(),
        Some("127.0.0.1" | "localhost" | "::1")
    ));
    let admin = PgPool::connect(&url).await.unwrap();
    let schema = format!("backstop_{}", uuid::Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&admin)
        .await
        .unwrap();
    let scope = schema.clone();
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(5)
        .after_connect(move |connection, _| {
            let scope = scope.clone();
            Box::pin(async move {
                sqlx::query(&format!("SET search_path={scope}"))
                    .execute(connection)
                    .await?;
                Ok(())
            })
        })
        .connect(&url)
        .await
        .unwrap();
    for ddl in [
        include_str!("../../../../tests/fixtures/legacy-catalog.sql"),
        jobs::MIGRATION,
        MIGRATION,
        include_str!("../../../../migrations/0012_provider_receipts.sql"),
    ] {
        sqlx::raw_sql(ddl).execute(&pool).await.unwrap();
    }
    sqlx::raw_sql("CREATE TABLE zuse_rust_task_command_execution(workspace_id text NOT NULL,command_id text NOT NULL,state text NOT NULL,updated_at bigint NOT NULL,runtime_generation bigint,session_id text NOT NULL DEFAULT 'session-bob',error_code text,unknown_reviewed boolean NOT NULL DEFAULT false,PRIMARY KEY(workspace_id,command_id)); CREATE TABLE zuse_rust_account_event_outbox(owner_id text NOT NULL,kind text NOT NULL,resource_id text NOT NULL,workspace_id text,payload jsonb NOT NULL,created_at bigint NOT NULL); INSERT INTO zuse_rust_control_owners VALUES('workspace','ws-bob','rust',2,0)").execute(&pool).await.unwrap();
    let control = control(pool.clone());
    let quiet = control.max_quiet_awake_ms();
    let idle_window = control.idle_ms + crate::runtime::activity::COALESCE_MS;
    // Recent activity: nothing sleeps. After one idle window: only the
    // ordinary path. After the bound: everything but the exceptions.
    let recent = T0 + 1_000;
    let after_idle = T0 + idle_window + 1_000;
    let after_bound = T0 + quiet + 1_000;
    assert!(after_idle < T0 + quiet);
    let mut failures = Vec::new();
    for row in matrix() {
        let mut check = |ok: bool, what: &str| {
            if !ok {
                failures.push(format!("{}: {what}", row.name));
            }
        };
        reset(&pool, &row).await;
        check(
            !control.idle_sleep_once(recent).await.unwrap() && slept(&pool).await.is_none(),
            "slept despite recent activity",
        );
        if row.expect == Lost {
            let lost = T0 + LOST_RUNTIME_GRACE_MS;
            let due = control.idle_sleep_due(recent).await.unwrap();
            check(due == Some(lost), &format!("lost-runtime deadline {due:?}"));
            check(
                control.idle_sleep_once(lost + 1_000).await.unwrap()
                    && slept(&pool).await.is_some(),
                "not slept soon after its runtime was lost",
            );
            let turn: (String, Option<String>, bool) = sqlx::query_as("SELECT state,error_code,unknown_reviewed FROM zuse_rust_task_command_execution WHERE workspace_id='ws-bob'")
                .fetch_one(&pool).await.unwrap();
            check(
                turn == (
                    "outcome-unknown".into(),
                    Some("runtime-process-lost".into()),
                    false,
                ),
                &format!("lost turn recorded as {turn:?}"),
            );
            let events: i64 = sqlx::query_scalar("SELECT count(*) FROM zuse_rust_account_event_outbox WHERE kind='task-command-progress' AND workspace_id='ws-bob'")
                .fetch_one(&pool).await.unwrap();
            check(events == 1, &format!("{events} review events"));
            continue;
        }
        let normal = control.idle_sleep_once(after_idle).await.unwrap();
        check(
            normal == (row.expect == Normal),
            &format!("ordinary idle admission was {normal}"),
        );
        if row.expect == Normal {
            continue;
        }
        let due = control.idle_sleep_due(after_bound).await.unwrap();
        let admitted = control.idle_sleep_once(after_bound).await.unwrap();
        match row.expect {
            Backstop => {
                check(
                    due.is_some_and(|at| at <= after_bound && at >= T0 + quiet),
                    &format!("backstop deadline {due:?} is not the bound"),
                );
                check(admitted, "not slept after the max quiet bound");
                let Some(workspace) = slept(&pool).await else {
                    check(false, "workspace not pausing");
                    continue;
                };
                let config = &workspace["request_config"];
                let operation = config["rustLifecycleOperation"]["id"]
                    .as_str()
                    .unwrap_or("");
                check(operation != OP, "stuck operation not superseded");
                check(
                    config["rustIdleBackstop"]["operationId"] == operation,
                    "backstop marker missing",
                );
                let alive = row.state == "ready"
                    && row.runtime == "online"
                    && row.patch["runtimeCredentialExpiresAtMs"] != 0;
                let phase: Option<String> = sqlx::query_scalar("SELECT phase FROM zuse_rust_lifecycle_operations WHERE operation_id=$1 AND action='pause' AND source_machine_id='machine-bob' AND storage_incarnation_id='retained-disk'")
                    .bind(operation).fetch_optional(&pool).await.unwrap();
                check(
                    phase.as_deref() == Some(if alive { "quiescing" } else { "quiesced" }),
                    &format!("pause operation phase {phase:?}"),
                );
                let jobs: Vec<(String, String)> = sqlx::query_as("SELECT payload->>'operationId',state FROM zuse_rust_jobs WHERE resource_id='ws-bob' ORDER BY created_at")
                    .fetch_all(&pool).await.unwrap();
                check(
                    jobs.iter()
                        .any(|(op, state)| op == operation && state == "pending")
                        && jobs
                            .iter()
                            .all(|(op, state)| op == operation || state == "failed"),
                    &format!("jobs {jobs:?}"),
                );
                check(
                    !control.idle_sleep_once(after_bound + 1).await.unwrap(),
                    "backstop fired twice",
                );
            }
            Never | Held(_) => {
                check(due.is_none(), &format!("deadline {due:?} would spin"));
                check(
                    !admitted && slept(&pool).await.is_none(),
                    "slept a computer it must not touch",
                );
                let far = control.idle_sleep_once(T0 + 100 * quiet).await.unwrap();
                check(!far, "slept much later");
                let holds: Vec<(String, i64, String)> =
                    sqlx::query_as(&awake("workspace_id,quiet_since,hold", "hold IS NOT NULL"))
                        .bind(control.creation_accounts.filter())
                        .bind(0_i64)
                        .bind(after_bound)
                        .bind(None::<String>)
                        .fetch_all(&pool)
                        .await
                        .unwrap();
                let want = match row.expect {
                    Held(hold) => hold,
                    _ => "live-turn",
                };
                check(
                    holds.len() == 1 && holds[0].2 == want && holds[0].1 == T0,
                    &format!("holds {holds:?}"),
                );
            }
            Normal | Lost => unreachable!(),
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
    pool.close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .unwrap();
    admin.close().await;
}

#[test]
fn held_computers_are_logged_once_per_transition() {
    let mut held = HashMap::new();
    let row = |id: &str, hold: &str| (id.to_owned(), 0, hold.to_owned());
    assert_eq!(
        held_transitions(&mut held, vec![row("a", "repair")], 10),
        ["a"]
    );
    assert!(held_transitions(&mut held, vec![row("a", "repair")], 20).is_empty());
    assert_eq!(
        held_transitions(
            &mut held,
            vec![row("a", "recreation"), row("b", "live-turn")],
            30
        ),
        ["a", "b"]
    );
    assert!(held_transitions(&mut held, vec![], 40).is_empty());
    assert!(held.is_empty());
}
