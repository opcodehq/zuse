use super::*;
use crate::{access::Scope, auth::Principal, crypto::MasterKey, tickets::TicketSigner};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD as B64};
use std::sync::{
    Mutex,
    atomic::{AtomicUsize, Ordering},
};
struct Fake {
    pool: PgPool,
    workspace: String,
    operation: String,
    mode: AtomicUsize,
    creates: AtomicUsize,
    launches: AtomicUsize,
    machine: Mutex<Option<Machine>>,
    observed_lease: Mutex<String>,
}
#[async_trait]
impl AllocationProvider for Fake {
    async fn find(&self, _: &str) -> Result<Option<Machine>, ProviderError> {
        if self.mode.load(Ordering::SeqCst) == 5 {
            let lease: String = sqlx::query_scalar(
                "SELECT lease_token FROM zuse_rust_jobs WHERE resource_id=$1 AND state='running'",
            )
            .bind(&self.workspace)
            .fetch_one(&self.pool)
            .await
            .unwrap();
            *self.observed_lease.lock().unwrap() = lease;
            return Err(ProviderError::Rejected);
        }
        Ok(self.machine.lock().unwrap().clone())
    }
    async fn inspect_id(&self, id: &str) -> Result<Option<Machine>, ProviderError> {
        assert_eq!(id, "original-machine");
        Ok(self.machine.lock().unwrap().clone())
    }
    async fn create(&self, _: &str, _: &str) -> Result<Machine, ProviderError> {
        panic!("explicit original operation required")
    }
    async fn create_operation(
        &self,
        snapshot: &str,
        name: &str,
        operation: &str,
    ) -> Result<Machine, ProviderError> {
        assert_eq!(operation, self.operation);
        assert_eq!(snapshot, "original-base");
        self.creates.fetch_add(1, Ordering::SeqCst);
        let lease: String = sqlx::query_scalar(
            "SELECT lease_token FROM zuse_rust_jobs WHERE resource_id=$1 AND state='running'",
        )
        .bind(&self.workspace)
        .fetch_one(&self.pool)
        .await
        .unwrap();
        *self.observed_lease.lock().unwrap() = lease;
        sqlx::query("UPDATE api_cloud_workspaces SET revision=revision+1 WHERE workspace_id=$1")
            .bind(&self.workspace)
            .execute(&self.pool)
            .await
            .unwrap();
        match self.mode.load(Ordering::SeqCst) {
            0 | 6 | 7 | 8 => Err(ProviderError::Rejected),
            2 => Err(ProviderError::Unavailable),
            3 => Err(ProviderError::RateLimited),
            mode => {
                let m = Machine {
                    id: "original-machine".into(),
                    name: name.into(),
                    state: MachineState::Running,
                    vcpu: None,
                    memory_bytes: None,
                };
                *self.machine.lock().unwrap() = Some(m.clone());
                if mode == 4 {
                    Err(ProviderError::OutcomeUnknown)
                } else {
                    Ok(m)
                }
            }
        }
    }
    async fn launch(&self, id: &str, launch: &RuntimeLaunch) -> Result<(), ProviderError> {
        assert_eq!(id, "original-machine");
        assert_eq!(launch.operation_id, self.operation);
        self.launches.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}
fn actor(owner: &str) -> Access {
    Access {
        actor: Principal {
            account_id: owner.into(),
        },
        scope: Scope::Personal,
        owner_id: owner.into(),
        membership: None,
    }
}
fn request() -> Retry {
    Retry {
        command_id: uuid::Uuid::new_v4().to_string(),
    }
}
#[tokio::test]
#[ignore = "requires explicit isolated localhost PostgreSQL"]
async fn permanent_create_rejection_requires_authorized_original_retry_and_preserves_uncertainty() {
    let url = std::env::var("ZUSE_CLOUD_TEST_DATABASE_URL").unwrap();
    assert!(matches!(
        reqwest::Url::parse(&url).unwrap().host_str(),
        Some("localhost" | "127.0.0.1" | "::1")
    ));
    let admin = PgPool::connect(&url).await.unwrap();
    for batched in [false, true] {
    for mode in [0, 5, 6, 7, 8, 2, 3, 4] {
        let schema = format!("reject_{}", uuid::Uuid::new_v4().simple());
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
            include_str!("../../../../tests/fixtures/legacy-catalog.sql"),
            jobs::MIGRATION,
            MIGRATION,
            super::super::super::deletion::MIGRATION,
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
        let key = ed25519_dalek::SigningKey::from_bytes(&[51; 32]);
        let jwk = json!({"kty":"OKP","crv":"Ed25519","d":B64.encode([51;32]),"x":B64.encode(key.verifying_key().as_bytes())});
        let control = RuntimeControl::from_pool(
            pool.clone(),
            MasterKey::parse(&B64.encode([52; 32])).unwrap(),
            TicketSigner::from_jwk(&jwk.to_string(), "https://api.example.test").unwrap(),
        ).with_fast_start_accounts(if batched { vec!["user-bob".into()] } else { vec![] });
        assert_eq!(control.statement_batch_available().await.unwrap(),batched);
        let op = control
            .queue_allocation(&id, "original-base", now)
            .await
            .unwrap();
        let fake = Fake {
            pool: pool.clone(),
            workspace: id.clone(),
            operation: op.clone(),
            mode: AtomicUsize::new(mode),
            creates: AtomicUsize::new(0),
            launches: AtomicUsize::new(0),
            machine: Mutex::new(None),
            observed_lease: Mutex::new(String::new()),
        };
        if mode == 8 {
            sqlx::raw_sql("CREATE FUNCTION expire_consumed_retry() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.outcome='rejected' AND EXISTS(SELECT 1 FROM api_cloud_workspaces w WHERE w.workspace_id=NEW.workspace_id AND w.request_config->'rustAllocationDispatchAuthorization'->>'state'='dispatched') THEN UPDATE zuse_rust_jobs SET lease_expires_at=0 WHERE resource_id=NEW.workspace_id AND state='running'; END IF; RETURN NEW; END $$; CREATE TRIGGER retry_rejected_then_crash AFTER UPDATE OF outcome ON zuse_rust_provider_creates FOR EACH ROW EXECUTE FUNCTION expire_consumed_retry();").execute(&pool).await.unwrap();
        }
        if mode == 6 || mode == 7 {
            sqlx::raw_sql("CREATE FUNCTION expire_rejected_lease() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.outcome='rejected' THEN UPDATE zuse_rust_jobs SET lease_expires_at=0 WHERE resource_id=NEW.workspace_id AND state='running'; END IF; RETURN NEW; END $$; CREATE TRIGGER reject_then_lose_lease AFTER UPDATE OF outcome ON zuse_rust_provider_creates FOR EACH ROW EXECUTE FUNCTION expire_rejected_lease();").execute(&pool).await.unwrap();
        }
        assert!(control.allocate_once(&fake, clock_ms() + 1).await.is_err());
        if mode == 6 || mode == 7 {
            let stale_state: String =
                sqlx::query_scalar("SELECT state FROM zuse_rust_jobs WHERE resource_id=$1")
                    .bind(&id)
                    .fetch_one(&pool)
                    .await
                    .unwrap();
            assert_eq!(
                stale_state, "running",
                "receipt committed before attention while original lease expired"
            );
            let rejected: String = sqlx::query_scalar(
                "SELECT outcome FROM zuse_rust_provider_creates WHERE operation_id=$1",
            )
            .bind(&op)
            .fetch_one(&pool)
            .await
            .unwrap();
            assert_eq!(rejected, "rejected");
            assert!(control.allocate_once(&fake, clock_ms() + 1).await.is_err());
            assert_eq!(
                fake.creates.load(Ordering::SeqCst),
                1,
                "fresh worker MUST NOT repost an independently committed rejection"
            );
        }
        let job: Value =
            sqlx::query_scalar("SELECT to_jsonb(j) FROM zuse_rust_jobs j WHERE resource_id=$1")
                .bind(&id)
                .fetch_one(&pool)
                .await
                .unwrap();
        if [0, 5, 6, 7, 8].contains(&mode) {
            let rejected_creates = usize::from(mode != 5);
            assert_eq!(job["state"], "failed");
            assert!(
                !control
                    .allocate_once(&fake, clock_ms() + 600_000)
                    .await
                    .unwrap()
            );
            assert_eq!(fake.creates.load(Ordering::SeqCst), rejected_creates);
            assert!(
                control
                    .retry_allocation(&id, &op, &actor("user-alice"), request(), clock_ms())
                    .await
                    .is_err()
            );
            assert!(
                control
                    .retry_allocation(
                        &id,
                        &uuid::Uuid::new_v4().to_string(),
                        &actor("user-bob"),
                        request(),
                        clock_ms()
                    )
                    .await
                    .is_err()
            );
            sqlx::query(
                "UPDATE zuse_rust_control_owners SET backend='legacy' WHERE resource_id=$1",
            )
            .bind(&id)
            .execute(&pool)
            .await
            .unwrap();
            assert!(
                control
                    .retry_allocation(&id, &op, &actor("user-bob"), request(), clock_ms())
                    .await
                    .is_err()
            );
            sqlx::query("UPDATE zuse_rust_control_owners SET backend='rust' WHERE resource_id=$1")
                .bind(&id)
                .execute(&pool)
                .await
                .unwrap();
            sqlx::query("UPDATE zuse_rust_control_owners SET generation=2 WHERE resource_id=$1")
                .bind(&id)
                .execute(&pool)
                .await
                .unwrap();
            assert!(
                control
                    .retry_allocation(&id, &op, &actor("user-bob"), request(), clock_ms())
                    .await
                    .is_err()
            );
            sqlx::query("UPDATE zuse_rust_control_owners SET generation=1 WHERE resource_id=$1")
                .bind(&id)
                .execute(&pool)
                .await
                .unwrap();
            sqlx::query(
                "UPDATE api_cloud_workspaces SET desired_state='deleted' WHERE workspace_id=$1",
            )
            .bind(&id)
            .execute(&pool)
            .await
            .unwrap();
            assert!(
                control
                    .retry_allocation(&id, &op, &actor("user-bob"), request(), clock_ms())
                    .await
                    .is_err()
            );
            sqlx::query(
                "UPDATE api_cloud_workspaces SET desired_state='ready' WHERE workspace_id=$1",
            )
            .bind(&id)
            .execute(&pool)
            .await
            .unwrap();
            sqlx::query(
                "UPDATE zuse_rust_allocations SET snapshot_id='changed-base' WHERE operation_id=$1",
            )
            .bind(&op)
            .execute(&pool)
            .await
            .unwrap();
            assert!(
                control
                    .retry_allocation(&id, &op, &actor("user-bob"), request(), clock_ms())
                    .await
                    .is_err()
            );
            sqlx::query("UPDATE zuse_rust_allocations SET snapshot_id='original-base',boot_expires_at=0 WHERE operation_id=$1").bind(&op).execute(&pool).await.unwrap();
            if mode == 5 {
                sqlx::query("INSERT INTO zuse_rust_provider_creates(operation_id,workspace_id,machine_name,outcome,updated_at) SELECT operation_id,workspace_id,machine_name,'unknown',$2 FROM zuse_rust_allocations WHERE operation_id=$1").bind(&op).bind(clock_ms()).execute(&pool).await.unwrap();
            }
            sqlx::query(
                "UPDATE zuse_rust_provider_creates SET outcome='unknown' WHERE operation_id=$1",
            )
            .bind(&op)
            .execute(&pool)
            .await
            .unwrap();
            assert!(
                control
                    .retry_allocation(&id, &op, &actor("user-bob"), request(), clock_ms())
                    .await
                    .is_err()
            );
            sqlx::query(
                "UPDATE zuse_rust_provider_creates SET outcome='rejected' WHERE operation_id=$1",
            )
            .bind(&op)
            .execute(&pool)
            .await
            .unwrap();
            if mode == 5 {
                sqlx::query("DELETE FROM zuse_rust_provider_creates WHERE operation_id=$1")
                    .bind(&op)
                    .execute(&pool)
                    .await
                    .unwrap();
            }
            sqlx::query("UPDATE zuse_rust_jobs SET state='running',lease_token='other-worker',lease_expires_at=$2 WHERE resource_id=$1").bind(&id).bind(clock_ms()+60_000).execute(&pool).await.unwrap();
            assert!(
                control
                    .retry_allocation(&id, &op, &actor("user-bob"), request(), clock_ms())
                    .await
                    .is_err()
            );
            sqlx::query("UPDATE zuse_rust_jobs SET state='failed',lease_token=NULL,lease_expires_at=NULL WHERE resource_id=$1").bind(&id).execute(&pool).await.unwrap();
            let command = uuid::Uuid::new_v4().to_string();
            let prior_record: Option<Value> = sqlx::query_scalar(
                "SELECT to_jsonb(c) FROM zuse_rust_provider_creates c WHERE operation_id=$1",
            )
            .bind(&op)
            .fetch_optional(&pool)
            .await
            .unwrap();
            let receipt = control
                .retry_allocation(
                    &id,
                    &op,
                    &actor("user-bob"),
                    Retry {
                        command_id: command.clone(),
                    },
                    clock_ms(),
                )
                .await
                .unwrap();
            assert_eq!(receipt["operationId"], op);
            assert_eq!(
                receipt["priorProviderReceipt"],
                prior_record.unwrap_or(Value::Null),
                "original rejected receipt must remain in immutable retry ledger"
            );
            let replay = control
                .retry_allocation(
                    &id,
                    &op,
                    &actor("user-bob"),
                    Retry {
                        command_id: command.clone(),
                    },
                    clock_ms(),
                )
                .await
                .unwrap();
            assert_eq!(receipt, replay);
            assert_eq!(fake.creates.load(Ordering::SeqCst), rejected_creates);
            let original:(String,String,String,i64)=sqlx::query_as("SELECT a.operation_id,a.machine_name,a.storage_incarnation_id,a.boot_expires_at FROM zuse_rust_allocations a WHERE workspace_id=$1").bind(&id).fetch_one(&pool).await.unwrap();
            assert_eq!(original.0, op);
            assert!(original.3 > clock_ms());
            assert_eq!(
                sqlx::query_scalar::<_, i64>(
                    "SELECT count(*) FROM zuse_rust_jobs WHERE resource_id=$1"
                )
                .bind(&id)
                .fetch_one(&pool)
                .await
                .unwrap(),
                1
            );
            let stale = jobs::Job {
                id: job["job_id"].as_str().unwrap().into(),
                kind: JOB_KIND.into(),
                resource_id: id.clone(),
                generation: 1,
                payload: json!({"operationId":op}),
                lease_token: fake.observed_lease.lock().unwrap().clone(),
                attempts: 1,
            };
            let held = jobs::claim(&pool, JOB_KIND, "workspace", clock_ms() + 1, 120_000)
                .await
                .unwrap()
                .unwrap();
            assert!(
                !control
                    .stop_rejected_allocation(&stale, clock_ms())
                    .await
                    .unwrap(),
                "a stale worker cannot fail the new retry lease"
            );
            let active: String = sqlx::query_scalar(
                "SELECT lease_token FROM zuse_rust_jobs WHERE resource_id=$1 AND state='running'",
            )
            .bind(&id)
            .fetch_one(&pool)
            .await
            .unwrap();
            assert_eq!(active, held.lease_token);
            if mode == 7 {
                let operation: Value = sqlx::query_scalar(
                    "SELECT to_jsonb(a) FROM zuse_rust_allocations a WHERE operation_id=$1",
                )
                .bind(&op)
                .fetch_one(&pool)
                .await
                .unwrap();
                control
                    .begin_allocation_dispatch(&held, &operation)
                    .await
                    .unwrap();
                let pending: String = sqlx::query_scalar(
                    "SELECT outcome FROM zuse_rust_provider_creates WHERE operation_id=$1",
                )
                .bind(&op)
                .fetch_one(&pool)
                .await
                .unwrap();
                assert_eq!(pending, "pending");
                let dispatched:String=sqlx::query_scalar("SELECT request_config#>>'{rustAllocationDispatchAuthorization,state}' FROM api_cloud_workspaces WHERE workspace_id=$1").bind(&id).fetch_one(&pool).await.unwrap();
                assert_eq!(dispatched, "dispatched");
                sqlx::query(
                    "UPDATE zuse_rust_jobs SET lease_expires_at=0,due_at=0 WHERE job_id=$1",
                )
                .bind(&held.id)
                .execute(&pool)
                .await
                .unwrap();
                fake.mode.store(1, Ordering::SeqCst);
                assert!(control.allocate_once(&fake, clock_ms() + 1).await.is_err());
                assert_eq!(
                    fake.creates.load(Ordering::SeqCst),
                    rejected_creates,
                    "crash after dispatch claim is uncertain, NEVER a new POST"
                );
                assert!(
                    control
                        .retry_allocation(&id, &op, &actor("user-bob"), request(), clock_ms())
                        .await
                        .is_err()
                );
            } else {
                if mode == 6 {
                    sqlx::query(
                        "UPDATE zuse_rust_jobs SET lease_expires_at=0,due_at=0 WHERE job_id=$1",
                    )
                    .bind(&held.id)
                    .execute(&pool)
                    .await
                    .unwrap();
                } else {
                    assert!(
                        jobs::retry(&pool, &held, clock_ms(), "fixture_lease_released")
                            .await
                            .unwrap()
                    );
                    sqlx::query("UPDATE zuse_rust_jobs SET due_at=0 WHERE resource_id=$1")
                        .bind(&id)
                        .execute(&pool)
                        .await
                        .unwrap();
                }
                if mode == 8 {
                    fake.mode.store(0, Ordering::SeqCst);
                    assert!(control.allocate_once(&fake, clock_ms() + 1).await.is_err());
                    assert_eq!(fake.creates.load(Ordering::SeqCst), 2);
                    assert!(control.allocate_once(&fake, clock_ms() + 1).await.is_err());
                    assert_eq!(
                        fake.creates.load(Ordering::SeqCst),
                        2,
                        "consumed old permit cannot repost a SECOND rejection after lease loss"
                    );
                    let old_replay = control
                        .retry_allocation(
                            &id,
                            &op,
                            &actor("user-bob"),
                            Retry {
                                command_id: command.clone(),
                            },
                            clock_ms(),
                        )
                        .await
                        .unwrap();
                    assert_eq!(old_replay, receipt);
                    assert!(
                        !control
                            .allocate_once(&fake, clock_ms() + 600_000)
                            .await
                            .unwrap()
                    );
                    control
                        .retry_allocation(&id, &op, &actor("user-bob"), request(), clock_ms())
                        .await
                        .unwrap();
                }
                fake.mode.store(1, Ordering::SeqCst);
                control.allocate_once(&fake, clock_ms() + 1).await.unwrap();
                assert_eq!(
                    fake.creates.load(Ordering::SeqCst),
                    rejected_creates + 1 + usize::from(mode == 8)
                );
                assert_eq!(
                    fake.machine.lock().unwrap().as_ref().unwrap().name,
                    original.1
                );
            }
        } else {
            assert_eq!(job["state"], "pending");
            assert!(
                control
                    .retry_allocation(&id, &op, &actor("user-bob"), request(), clock_ms())
                    .await
                    .is_err()
            );
            assert_eq!(fake.creates.load(Ordering::SeqCst), 1);
            if mode == 4 {
                sqlx::query("UPDATE zuse_rust_jobs SET due_at=0 WHERE resource_id=$1")
                    .bind(&id)
                    .execute(&pool)
                    .await
                    .unwrap();
                control.allocate_once(&fake, clock_ms() + 1).await.unwrap();
                assert_eq!(fake.creates.load(Ordering::SeqCst), 1);
            }
        }
        let launch: (String,String)=sqlx::query_as("SELECT command_id,ciphertext FROM api_cloud_workspace_launch_intents WHERE workspace_id=$1").bind(&id).fetch_one(&pool).await.unwrap();
        assert_eq!(
            launch,
            ("original-cid".into(), "original-sealed-intent".into())
        );
        drop(control);
        drop(fake);
        pool.close().await;
        sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
            .execute(&admin)
            .await
            .unwrap();
    }
    }
}
