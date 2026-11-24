//! REGRESSION GUARD for "Preparing cloud computer… forever".
//!
//! Invariant: a first start (allocation without a provider machine) always
//! reaches one of
//!   * a machine: created, or adopted by its exact name;
//!   * a new create under the same name, once an uncertain create has stayed
//!     invisible by name for `UNRESOLVED_CREATE_WINDOW_MS`;
//!   * a visible failure (`allocationRecovery`, status
//!     `cloud_provider_rejected`) with a working Retry: immediately for a
//!     definite provider refusal, otherwise once
//!     `ALLOCATION_START_DEADLINE_MS` has passed since the start (or its last
//!     Retry);
//! and a definite refusal is never journaled as `unknown`. Archive of a first
//! start that never finished cancels it through deletion instead of "busy".
//!
//! Every row below is a stuck combination seen in production or in the code.
//! Changing the create journal, dispatch or stop rules must keep this green;
//! add a row for any new condition that can hold a first start.
use super::*;
use crate::{access::Scope, auth::Principal, crypto::MasterKey, tickets::TicketSigner};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD as B64};
use rejection::Retry;
use std::sync::{
    Mutex,
    atomic::{AtomicUsize, Ordering},
};

#[derive(Clone, Copy, Debug, PartialEq)]
enum Create {
    Ok,
    Rejected,
    /// gRPC NOT_FOUND: the snapshot is not visible on this endpoint.
    NotFound,
    RateLimited,
    /// Lost response; the machine exists anyway.
    LostButCreated,
}
#[derive(Clone, Copy, Debug, PartialEq)]
enum Find {
    /// Name lookup works; answers with whatever machine exists.
    Works,
    /// The provider is unreachable for lookups.
    Fails,
}
struct Fake {
    create: Mutex<Create>,
    find: Mutex<Find>,
    creates: AtomicUsize,
    machine: Mutex<Option<Machine>>,
}
impl Fake {
    fn new(create: Create, find: Find) -> Self {
        Self {
            create: Mutex::new(create),
            find: Mutex::new(find),
            creates: AtomicUsize::new(0),
            machine: Mutex::new(None),
        }
    }
    fn creates(&self) -> usize {
        self.creates.load(Ordering::SeqCst)
    }
}
#[async_trait]
impl AllocationProvider for Fake {
    async fn find(&self, name: &str) -> Result<Option<Machine>, ProviderError> {
        if *self.find.lock().unwrap() == Find::Fails {
            return Err(ProviderError::Unavailable);
        }
        Ok(self
            .machine
            .lock()
            .unwrap()
            .clone()
            .filter(|m| m.name == name))
    }
    async fn inspect_id(&self, id: &str) -> Result<Option<Machine>, ProviderError> {
        Ok(self.machine.lock().unwrap().clone().filter(|m| m.id == id))
    }
    async fn create(&self, _: &str, _: &str) -> Result<Machine, ProviderError> {
        panic!("explicit original operation required")
    }
    async fn create_operation(
        &self,
        _snapshot: &str,
        name: &str,
        _operation: &str,
    ) -> Result<Machine, ProviderError> {
        self.creates.fetch_add(1, Ordering::SeqCst);
        let machine = Machine {
            id: "machine-1".into(),
            name: name.into(),
            state: MachineState::Starting,
            vcpu: None,
            memory_bytes: None,
        };
        match *self.create.lock().unwrap() {
            Create::Rejected => Err(ProviderError::Rejected),
            Create::NotFound => Err(ProviderError::NotFound),
            Create::RateLimited => Err(ProviderError::RateLimited),
            Create::LostButCreated => {
                *self.machine.lock().unwrap() = Some(machine);
                Err(ProviderError::OutcomeUnknown)
            }
            Create::Ok => {
                *self.machine.lock().unwrap() = Some(machine.clone());
                Ok(machine)
            }
        }
    }
    async fn launch(&self, _: &str, _: &RuntimeLaunch) -> Result<(), ProviderError> {
        Ok(())
    }
}
struct PurgeableObjects;
#[async_trait]
impl crate::objects::Objects for PurgeableObjects {
    async fn get(&self, _: &str) -> Result<Option<Vec<u8>>, ApiError> {
        Ok(None)
    }
    async fn put(&self, _: &str, _: &[u8]) -> Result<(), ApiError> {
        Ok(())
    }
    fn supports_purge(&self) -> bool {
        true
    }
    async fn purge_prefix(&self, _: &str) -> Result<bool, ApiError> {
        Ok(true)
    }
}
fn actor() -> crate::access::Access {
    crate::access::Access {
        actor: Principal {
            account_id: "user-bob".into(),
        },
        scope: Scope::Personal,
        owner_id: "user-bob".into(),
        membership: None,
    }
}

struct Fixture {
    admin: PgPool,
    schema: String,
    pool: PgPool,
    control: RuntimeControl,
    id: String,
    op: String,
}
impl Fixture {
    async fn new(batched: bool) -> Self {
        let url = std::env::var("ZUSE_CLOUD_TEST_DATABASE_URL").unwrap();
        assert!(matches!(
            reqwest::Url::parse(&url).unwrap().host_str(),
            Some("localhost" | "127.0.0.1" | "::1")
        ));
        let admin = PgPool::connect(&url).await.unwrap();
        let schema = format!("start_deadline_{}", uuid::Uuid::new_v4().simple());
        sqlx::query(&format!("CREATE SCHEMA {schema}"))
            .execute(&admin)
            .await
            .unwrap();
        let namespace = schema.clone();
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(4)
            .after_connect(move |c, _| {
                let namespace = namespace.clone();
                Box::pin(async move {
                    sqlx::query(&format!("SET search_path={namespace}"))
                        .execute(c)
                        .await?;
                    Ok(())
                })
            })
            .connect(&url)
            .await
            .unwrap();
        for migration in [
            include_str!("../../../tests/fixtures/legacy-catalog.sql"),
            jobs::MIGRATION,
            MIGRATION,
            super::super::deletion::MIGRATION,
            crate::gateway::MIGRATION,
            super::super::lifecycle::MIGRATION,
        ] {
            sqlx::raw_sql(migration).execute(&pool).await.unwrap();
        }
        if batched {
            crate::runtime::statement_batch::fixture_schema(&pool).await;
        }
        let id = format!("ws-{}", uuid::Uuid::new_v4().simple());
        let now = clock_ms();
        sqlx::query("UPDATE api_cloud_workspaces SET workspace_id=$1,provider='boxd',provider_sandbox_id=NULL,runtime_credential_hash=NULL,runtime_boot_token_hash=NULL,state='queued',desired_state='ready',runtime_state='offline',request_config=$2 WHERE workspace_id='ws-bob'").bind(&id).bind(json!({"runtimeGeneration":1,"gatewayEpoch":1})).execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO zuse_rust_control_owners VALUES('workspace',$1,'rust',1,$2)")
            .bind(&id)
            .bind(now)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO api_cloud_workspace_launch_intents VALUES($1,'user-bob','chat','session','turn','original-cid','original-sealed-intent',$2,$3)").bind(&id).bind(now+3_600_000).bind(now).execute(&pool).await.unwrap();
        let key = ed25519_dalek::SigningKey::from_bytes(&[61; 32]);
        let jwk = json!({"kty":"OKP","crv":"Ed25519","d":B64.encode([61;32]),"x":B64.encode(key.verifying_key().as_bytes())});
        let control = RuntimeControl::from_pool(
            pool.clone(),
            MasterKey::parse(&B64.encode([62; 32])).unwrap(),
            TicketSigner::from_jwk(&jwk.to_string(), "https://api.example.test").unwrap(),
        )
        .with_fast_start_accounts(if batched {
            vec!["user-bob".into()]
        } else {
            vec![]
        })
        .with_transcript_objects(std::sync::Arc::new(PurgeableObjects));
        assert_eq!(control.statement_batch_available().await.unwrap(), batched);
        let op = control
            .queue_allocation(&id, "original-base", now)
            .await
            .unwrap();
        Self {
            admin,
            schema,
            pool,
            control,
            id,
            op,
        }
    }
    /// `$1` workspace, `$2` operation.
    async fn exec(&self, sql: &str) {
        let query = sqlx::query(sql).bind(&self.id);
        let query = if sql.contains("$2") {
            query.bind(&self.op)
        } else {
            query
        };
        query.execute(&self.pool).await.unwrap();
    }
    /// One worker attempt as soon as the job may run.
    async fn attempt(&self, provider: &Fake) -> Result<bool, ApiError> {
        self.exec("UPDATE zuse_rust_jobs SET due_at=0 WHERE resource_id=$1 AND state='pending'")
            .await;
        self.control.allocate_once(provider, clock_ms() + 1).await
    }
    async fn journal(&self) -> Option<(String, Option<String>)> {
        sqlx::query_as(
            "SELECT outcome,machine_id FROM zuse_rust_provider_creates WHERE operation_id=$1",
        )
        .bind(&self.op)
        .fetch_optional(&self.pool)
        .await
        .unwrap()
    }
    async fn job_state(&self) -> String {
        sqlx::query_scalar("SELECT state FROM zuse_rust_jobs WHERE resource_id=$1 AND kind=$2")
            .bind(&self.id)
            .bind(JOB_KIND)
            .fetch_one(&self.pool)
            .await
            .unwrap()
    }
    async fn row(&self) -> Value {
        sqlx::query_scalar("SELECT to_jsonb(w) FROM api_cloud_workspaces w WHERE workspace_id=$1")
            .bind(&self.id)
            .fetch_one(&self.pool)
            .await
            .unwrap()
    }
    async fn machine(&self) -> Option<String> {
        sqlx::query_scalar("SELECT machine_id FROM zuse_rust_allocations WHERE operation_id=$1")
            .bind(&self.op)
            .fetch_one(&self.pool)
            .await
            .unwrap()
    }
    /// The state an earlier attempt left: allocation creating, this journal.
    async fn seed(&self, outcome: &str, age_ms: i64) {
        self.exec("UPDATE zuse_rust_allocations SET phase='creating' WHERE workspace_id=$1 AND operation_id=$2")
            .await;
        sqlx::query("INSERT INTO zuse_rust_provider_creates(operation_id,workspace_id,machine_name,outcome,updated_at) SELECT operation_id,workspace_id,machine_name,$2,$3 FROM zuse_rust_allocations WHERE operation_id=$1")
            .bind(&self.op).bind(outcome).bind(clock_ms() - age_ms).execute(&self.pool).await.unwrap();
    }
    async fn age_start(&self, age_ms: i64) {
        sqlx::query("UPDATE zuse_rust_allocations SET created_at=$2 WHERE operation_id=$1")
            .bind(&self.op)
            .bind(clock_ms() - age_ms)
            .execute(&self.pool)
            .await
            .unwrap();
    }
    async fn retry(&self) -> Result<Value, ApiError> {
        self.control
            .retry_allocation(
                &self.id,
                &self.op,
                &actor(),
                Retry {
                    command_id: uuid::Uuid::new_v4().to_string(),
                },
                clock_ms(),
            )
            .await
    }
    async fn close(self) {
        drop(self.control);
        self.pool.close().await;
        sqlx::query(&format!("DROP SCHEMA {} CASCADE", self.schema))
            .execute(&self.admin)
            .await
            .unwrap();
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Expect {
    /// The machine is created or adopted on this attempt; no extra create.
    Ready { creates: usize },
    /// Still resolving within its window: no create, job retries later.
    Waits,
    /// Visible failure with Retry; Retry then reaches a machine.
    Visible {
        reason: &'static str,
        creates: usize,
    },
}
use Expect::*;
const MIN: i64 = 60_000;
struct Case {
    name: &'static str,
    create: Create,
    find: Find,
    /// Journal an earlier attempt left: (outcome, age).
    seeded: Option<(&'static str, i64)>,
    /// How long ago the start began.
    start_age: i64,
    expect: Expect,
}
fn case(name: &'static str, create: Create, expect: Expect) -> Case {
    Case {
        name,
        create,
        find: Find::Works,
        seeded: None,
        start_age: 0,
        expect,
    }
}
fn matrix() -> Vec<Case> {
    vec![
        case("create succeeds", Create::Ok, Ready { creates: 1 }),
        case(
            "create NOT_FOUND: base snapshot missing on this endpoint",
            Create::NotFound,
            Visible {
                reason: "rejected",
                creates: 1,
            },
        ),
        case(
            "create refused",
            Create::Rejected,
            Visible {
                reason: "rejected",
                creates: 1,
            },
        ),
        case(
            "create response lost but the machine exists",
            Create::LostButCreated,
            Waits,
        ),
        case("create rate limited", Create::RateLimited, Waits),
        Case {
            seeded: Some(("unknown", MIN)),
            ..case(
                "uncertain create inside the resolution window",
                Create::Ok,
                Waits,
            )
        },
        Case {
            seeded: Some(("unknown", 4 * MIN)),
            start_age: 4 * MIN,
            ..case(
                "uncertain create invisible by name after the window",
                Create::Ok,
                Ready { creates: 1 },
            )
        },
        Case {
            seeded: Some(("pending", 4 * MIN)),
            start_age: 4 * MIN,
            ..case(
                "worker died after the dispatch claim",
                Create::Ok,
                Ready { creates: 1 },
            )
        },
        Case {
            seeded: Some(("rejected", 4 * MIN)),
            start_age: 4 * MIN,
            ..case(
                "rejected journal (operator edit after the incident)",
                Create::Ok,
                Visible {
                    reason: "rejected",
                    creates: 0,
                },
            )
        },
        Case {
            seeded: Some(("unknown", 4 * MIN)),
            start_age: 4 * MIN,
            find: Find::Fails,
            ..case("provider unreachable for name lookups", Create::Ok, Waits)
        },
        Case {
            seeded: Some(("unknown", 11 * MIN)),
            start_age: 11 * MIN,
            find: Find::Fails,
            ..case(
                "provider unreachable past the start deadline",
                Create::Ok,
                Visible {
                    reason: "deadline",
                    creates: 0,
                },
            )
        },
        Case {
            seeded: Some(("unknown", MIN)),
            start_age: 11 * MIN,
            ..case(
                "uncertain create inside its window past the start deadline",
                Create::Ok,
                Visible {
                    reason: "deadline",
                    creates: 0,
                },
            )
        },
        Case {
            find: Find::Fails,
            start_age: 11 * MIN,
            ..case(
                "never dispatched, lookups failing past the deadline",
                Create::Ok,
                Visible {
                    reason: "deadline",
                    creates: 0,
                },
            )
        },
    ]
}

async fn run(case: &Case, batched: bool) {
    let label = format!("{} (batched={batched})", case.name);
    let f = Fixture::new(batched).await;
    let fake = Fake::new(case.create, case.find);
    if let Some((outcome, age)) = case.seeded {
        f.seed(outcome, age).await;
    }
    f.age_start(case.start_age).await;
    let first = f.attempt(&fake).await;
    // Invariant (a): a definite refusal is never journaled as unknown.
    if matches!(case.create, Create::NotFound | Create::Rejected) && case.seeded.is_none() {
        assert_eq!(
            f.journal().await,
            Some(("rejected".into(), None)),
            "{label}: definite refusal"
        );
    }
    match case.expect {
        Ready { creates } => {
            first.unwrap_or_else(|e| panic!("{label}: {}", e.code));
            assert_eq!(f.machine().await.as_deref(), Some("machine-1"), "{label}");
            assert_eq!(fake.creates(), creates, "{label}");
        }
        Waits => {
            assert!(first.is_err(), "{label}: expected a retryable error");
            assert_eq!(f.job_state().await, "pending", "{label}");
            assert!(
                f.row().await["request_config"]["rustAllocationRejection"].is_null(),
                "{label}: not failed yet"
            );
            let created = fake.creates();
            // Bounded: past the deadline the same state stops visibly,
            // unless it resolved to a machine first.
            f.age_start(11 * MIN).await;
            let _ = f.attempt(&fake).await;
            if f.machine().await.is_none() {
                assert_eq!(f.job_state().await, "failed", "{label}: deadline");
                assert_eq!(
                    crate::runtime::allocation::rejection::metadata(&f.row().await)["reason"],
                    "deadline",
                    "{label}"
                );
            } else {
                assert!(fake.creates() <= created.max(1), "{label}: one machine");
            }
        }
        Visible { reason, creates } => {
            assert!(first.is_err(), "{label}");
            assert_eq!(fake.creates(), creates, "{label}");
            assert_eq!(f.job_state().await, "failed", "{label}");
            let row = f.row().await;
            assert_eq!(row["status_code"], "cloud_provider_rejected", "{label}");
            let recovery = crate::runtime::allocation::rejection::metadata(&row);
            assert_eq!(recovery["retryAvailable"], true, "{label}");
            assert_eq!(recovery["errorCode"], "cloud_provider_rejected", "{label}");
            assert_eq!(recovery["reason"], reason, "{label}");
            // Stopped means stopped: no further attempt is claimed.
            assert!(
                !f.attempt(&fake).await.unwrap(),
                "{label}: job stays failed"
            );
            // Retry works from the visible failure and reaches a machine.
            *fake.create.lock().unwrap() = Create::Ok;
            *fake.find.lock().unwrap() = Find::Works;
            f.retry()
                .await
                .unwrap_or_else(|e| panic!("{label}: retry {}", e.code));
            assert_eq!(f.job_state().await, "pending", "{label}");
            // A Retry never overrides the resolution window of an uncertain
            // create that may still be in flight.
            let young: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM zuse_rust_provider_creates WHERE operation_id=$1 AND outcome IN ('unknown','pending') AND updated_at>$2)")
                .bind(&f.op).bind(clock_ms() - effects::UNRESOLVED_CREATE_WINDOW_MS).fetch_one(&f.pool).await.unwrap();
            if young {
                assert!(f.attempt(&fake).await.is_err(), "{label}: in window");
                assert_eq!(fake.creates(), creates, "{label}: no create in window");
                assert_eq!(
                    f.job_state().await,
                    "pending",
                    "{label}: retry restarts the deadline"
                );
                f.exec("UPDATE zuse_rust_provider_creates SET updated_at=updated_at-240000 WHERE workspace_id=$1 AND operation_id=$2")
                    .await;
            }
            f.attempt(&fake)
                .await
                .unwrap_or_else(|e| panic!("{label}: after retry {}", e.code));
            assert_eq!(f.machine().await.as_deref(), Some("machine-1"), "{label}");
            assert_eq!(fake.creates(), creates + 1, "{label}: one new create");
        }
    }
    f.close().await;
}

#[tokio::test]
#[ignore = "requires explicit isolated localhost PostgreSQL"]
async fn first_start_never_hangs_in_any_stuck_state() {
    for batched in [false, true] {
        for case in matrix() {
            run(&case, batched).await;
        }
    }
}

#[tokio::test]
#[ignore = "requires explicit isolated localhost PostgreSQL"]
async fn a_late_original_create_is_adopted_not_duplicated() {
    // The lost create materialises before the window ends: the next attempt
    // adopts it by name and never creates again.
    let f = Fixture::new(false).await;
    let fake = Fake::new(Create::LostButCreated, Find::Works);
    assert!(f.attempt(&fake).await.is_err());
    assert_eq!(f.journal().await, Some(("unknown".into(), None)));
    f.attempt(&fake).await.unwrap();
    assert_eq!(f.machine().await.as_deref(), Some("machine-1"));
    assert_eq!(fake.creates(), 1);
    assert_eq!(
        f.journal().await,
        Some(("confirmed".into(), Some("machine-1".into())))
    );
    f.close().await;
}

#[tokio::test]
#[ignore = "requires explicit isolated localhost PostgreSQL"]
async fn archive_or_delete_cancels_a_stuck_first_start() {
    for action in ["archive", "delete"] {
        for stopped in [false, true] {
            let f = Fixture::new(false).await;
            let fake = Fake::new(Create::Ok, Find::Works);
            f.seed("unknown", MIN).await;
            if stopped {
                f.age_start(11 * MIN).await;
            }
            assert!(f.attempt(&fake).await.is_err());
            assert_eq!(
                f.job_state().await,
                if stopped { "failed" } else { "pending" }
            );
            let command = uuid::Uuid::new_v4().to_string();
            let body = if action == "archive" {
                f.control
                    .lifecycle_action(
                        &f.id,
                        &actor(),
                        zuse_protocol::cloud_lifecycle::Action::Archive,
                        Some(&command),
                        clock_ms(),
                    )
                    .await
            } else {
                f.control
                    .delete_workspace(&f.id, &actor(), Some(&command), clock_ms())
                    .await
            }
            .unwrap_or_else(|e| panic!("{action} stopped={stopped}: {}", e.code));
            assert_eq!(body["state"], "deleting", "{action}");
            // The allocation is cancelled: nothing claims it again.
            assert_eq!(f.job_state().await, "completed", "{action}");
            assert!(!f.attempt(&fake).await.unwrap());
            assert_eq!(fake.creates(), 0);
            // The uncertain create is a deletion target by its exact name.
            let deletion: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM zuse_rust_jobs WHERE resource_id=$1 AND kind=$2 AND state='pending'",
            )
            .bind(&f.id)
            .bind(super::super::deletion::JOB_KIND)
            .fetch_one(&f.pool)
            .await
            .unwrap();
            assert_eq!(deletion, 1, "{action}");
            // A replay of the same request returns the same deletion.
            if action == "archive" {
                let replay = f
                    .control
                    .lifecycle_action(
                        &f.id,
                        &actor(),
                        zuse_protocol::cloud_lifecycle::Action::Archive,
                        Some(&command),
                        clock_ms(),
                    )
                    .await
                    .unwrap();
                assert_eq!(replay["state"], "deleting");
            }
            f.close().await;
        }
    }
}

#[tokio::test]
#[ignore = "requires explicit isolated localhost PostgreSQL"]
async fn a_started_machine_that_never_enrolls_stops_visibly_and_retry_restarts_it() {
    let f = Fixture::new(false).await;
    let fake = Fake::new(Create::Ok, Find::Works);
    let machine = Machine {
        id: "machine-1".into(),
        name: sqlx::query_scalar(
            "SELECT machine_name FROM zuse_rust_allocations WHERE operation_id=$1",
        )
        .bind(&f.op)
        .fetch_one(&f.pool)
        .await
        .unwrap(),
        state: MachineState::Starting,
        vcpu: None,
        memory_bytes: None,
    };
    *fake.machine.lock().unwrap() = Some(machine);
    // The machine exists and the launch went out, but no runtime ever enrolled
    // (a helper that kept refusing, say) for longer than the start deadline.
    f.exec("UPDATE zuse_rust_allocations SET phase='launching',machine_id='machine-1' WHERE workspace_id=$1 AND operation_id=$2").await;
    f.exec("UPDATE api_cloud_workspaces SET state='provisioning',provider_sandbox_id='machine-1' WHERE workspace_id=$1").await;
    f.age_start(11 * MIN).await;
    f.attempt(&fake).await.unwrap();
    assert_eq!(f.job_state().await, "failed");
    let row = f.row().await;
    assert_eq!(row["status_code"], "cloud_provider_rejected");
    let shown = crate::runtime::allocation::rejection::metadata(&row);
    assert_eq!(shown["retryAvailable"], true);
    assert_eq!(shown["reason"], "deadline");
    assert_eq!(
        row["request_config"]["rustAllocationRejection"]["machineId"],
        "machine-1"
    );
    // Retry runs the start again on the same machine: no create, and the
    // deadline restarts from the Retry instead of stopping it at once.
    f.retry().await.unwrap();
    assert_eq!(f.job_state().await, "pending");
    let _ = f.attempt(&fake).await;
    let row = f.row().await;
    assert!(row["request_config"]["rustAllocationRejection"].is_null());
    assert_ne!(f.job_state().await, "failed");
    assert_eq!(fake.creates(), 0);
    assert_eq!(f.machine().await.as_deref(), Some("machine-1"));
    f.close().await;
}

/// A launch command that keeps "succeeding" without a runtime ever enrolling
/// (the launcher exits 0, its runtime dies) is relaunched every two seconds.
/// Those relaunches must not use up the job's attempts before the start
/// deadline: an exhausted job is never claimed again, so the deadline would
/// never fire and the computer would show "Starting" forever.
#[tokio::test]
#[ignore = "requires explicit isolated localhost PostgreSQL"]
async fn relaunches_that_never_enroll_still_reach_the_start_deadline() {
    let f = Fixture::new(false).await;
    let fake = Fake::new(Create::Ok, Find::Works);
    let machine = Machine {
        id: "machine-1".into(),
        name: sqlx::query_scalar(
            "SELECT machine_name FROM zuse_rust_allocations WHERE operation_id=$1",
        )
        .bind(&f.op)
        .fetch_one(&f.pool)
        .await
        .unwrap(),
        state: MachineState::Running,
        vcpu: None,
        memory_bytes: None,
    };
    *fake.machine.lock().unwrap() = Some(machine);
    f.exec("UPDATE zuse_rust_allocations SET phase='launching',machine_id='machine-1' WHERE workspace_id=$1 AND operation_id=$2").await;
    f.exec("UPDATE api_cloud_workspaces SET state='provisioning',provider_sandbox_id='machine-1' WHERE workspace_id=$1").await;
    for pass in 0..150 {
        assert!(
            f.attempt(&fake).await.unwrap(),
            "pass {pass}: the start was abandoned before its deadline"
        );
        assert_eq!(f.job_state().await, "pending");
    }
    f.age_start(11 * MIN).await;
    f.attempt(&fake).await.unwrap();
    assert_eq!(f.job_state().await, "failed");
    let row = f.row().await;
    assert_eq!(row["status_code"], "cloud_provider_rejected");
    assert_eq!(
        crate::runtime::allocation::rejection::metadata(&row)["reason"],
        "deadline"
    );
    f.close().await;
}
