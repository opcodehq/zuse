use super::*;
use crate::{crypto::MasterKey, tickets::TicketSigner};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use std::sync::atomic::{AtomicUsize, Ordering};

fn enrolled(workspace: &str, operation: &str, disk: &str, stage: &str) -> Value {
    json!({
        "runtimeGeneration":1,"gatewayEpoch":1,
        "runtimeRpcProtocol":zuse_protocol::cloud_gateway::RUST_RPC,
        "rustPreparationVersion":1,"rustAllocationOperationId":operation,
        "runtimeCredentialKeyThumbprint":"credential-key","runtimeSigningKeyThumbprint":"signing-key",
        "runtimeBootstrapReceipt":{"workspaceId":workspace,"generation":1,"gatewayEpoch":1},
        "rustRuntimeStorage":{"storageIncarnationId":disk},
        "rustStorageEnrollment":{"phase":"committed","generation":1,"sourceSandboxId":"original",
            "storageIncarnationId":disk,"credentialKeyThumbprint":"credential-key","signingKeyThumbprint":"signing-key"},
        "rustPreparationIngress":{"version":1,"operationId":operation,"generation":1,
            "sourceMachineId":"original","storageIncarnationId":disk},
        "preparation":{"repository":{"state":stage,"operationId":operation}}
    })
}

#[test]
fn only_current_committed_preparation_owns_initial_launch() {
    let config = enrolled("workspace", "operation", "disk", "failed");
    let row = json!({"workspace_id":"workspace","provider_sandbox_id":"original","state":"setup",
        "desired_state":"ready","runtime_state":"connecting","request_config":config});
    let operation = json!({"workspace_id":"workspace","operation_id":"operation","generation":1,
        "machine_id":"original","storage_incarnation_id":"disk","phase":"launching"});
    assert!(active(&row, &operation));
    for (path, replacement) in [
        (
            "/request_config/rustPreparationIngress/operationId",
            json!("previous"),
        ),
        (
            "/request_config/rustPreparationIngress/generation",
            json!(2),
        ),
        (
            "/request_config/rustPreparationIngress/sourceMachineId",
            json!("replacement"),
        ),
        (
            "/request_config/rustPreparationIngress/storageIncarnationId",
            json!("other"),
        ),
        (
            "/request_config/rustStorageEnrollment/phase",
            json!("prepared"),
        ),
        (
            "/request_config/runtimeBootstrapReceipt/generation",
            json!(2),
        ),
        ("/desired_state", json!("deleted")),
        ("/runtime_state", json!("online")),
    ] {
        let mut stale = row.clone();
        *stale.pointer_mut(path).unwrap() = replacement;
        assert!(!active(&stale, &operation), "{path}");
    }
    let mut wrong_source = operation.clone();
    wrong_source["machine_id"] = json!("replacement");
    assert!(!active(&row, &wrong_source));
}

struct Preparing {
    pool: PgPool,
    workspace: String,
    config: Value,
    inject_at: &'static str,
    inspections: AtomicUsize,
    readiness: AtomicUsize,
    launches: AtomicUsize,
}
impl Preparing {
    async fn inject(&self) {
        sqlx::query("UPDATE api_cloud_workspaces SET state='setup',runtime_state='connecting',status_code='repository_preparation_failed',runtime_credential_hash='enrolled',request_config=$2 WHERE workspace_id=$1")
            .bind(&self.workspace).bind(&self.config).execute(&self.pool).await.unwrap();
    }
}
#[async_trait]
impl AllocationProvider for Preparing {
    async fn find(&self, _: &str) -> Result<Option<Machine>, ProviderError> {
        panic!("Known allocation must never create/find a replacement")
    }
    async fn inspect_id(&self, id: &str) -> Result<Option<Machine>, ProviderError> {
        assert_eq!(id, "original");
        self.inspections.fetch_add(1, Ordering::SeqCst);
        if self.inject_at == "inspection" {
            self.inject().await;
        }
        let name: String = sqlx::query_scalar(
            "SELECT machine_name FROM zuse_rust_allocations WHERE workspace_id=$1",
        )
        .bind(&self.workspace)
        .fetch_one(&self.pool)
        .await
        .unwrap();
        Ok(Some(Machine {
            id: id.into(),
            name,
            state: MachineState::Running,
            vcpu: None,
            memory_bytes: None,
        }))
    }
    async fn runtime_ready(&self, _: &str) -> Result<bool, ProviderError> {
        self.readiness.fetch_add(1, Ordering::SeqCst);
        if self.inject_at == "readiness" {
            self.inject().await;
        }
        Ok(true)
    }
    async fn create(&self, _: &str, _: &str) -> Result<Machine, ProviderError> {
        panic!("Preparation must not allocate a computer")
    }
    async fn launch(&self, _: &str, _: &RuntimeLaunch) -> Result<(), ProviderError> {
        self.launches.fetch_add(1, Ordering::SeqCst);
        panic!("Preparation must not upload credentials or repeat launch")
    }
}

#[tokio::test]
#[ignore = "requires explicit isolated localhost PostgreSQL"]
async fn preparation_holds_preserve_errors_retry_budget_and_fence_inflight_inspections() {
    let url = std::env::var("ZUSE_CLOUD_TEST_DATABASE_URL").expect("isolated database required");
    assert!(matches!(
        reqwest::Url::parse(&url).unwrap().host_str(),
        Some("127.0.0.1" | "localhost" | "::1")
    ));
    let admin = PgPool::connect(&url).await.unwrap();
    for (inject_at, fault) in [
        ("existing", ""),
        ("inspection", ""),
        ("readiness", ""),
        ("existing", "expired-lease"),
        ("existing", "replaced-lease"),
        ("existing", "delete"),
        ("existing", "generation"),
    ] {
        let unique = uuid::Uuid::new_v4().simple().to_string();
        let schema = format!("preparation_{unique}");
        sqlx::query(&format!("CREATE SCHEMA {schema}"))
            .execute(&admin)
            .await
            .unwrap();
        let scope = schema.clone();
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(4)
            .after_connect(move |c, _| {
                let scope = scope.clone();
                Box::pin(async move {
                    sqlx::query(&format!("SET search_path={scope}"))
                        .execute(c)
                        .await?;
                    Ok(())
                })
            })
            .connect(&url)
            .await
            .unwrap();
        for sql in [
            include_str!("../../../../tests/fixtures/legacy-catalog.sql"),
            jobs::MIGRATION,
            MIGRATION,
        ] {
            sqlx::raw_sql(sql).execute(&pool).await.unwrap();
        }
        let workspace = format!("ws-{unique}");
        let now = clock_ms();
        sqlx::query("UPDATE api_cloud_workspaces SET workspace_id=$1,provider_sandbox_id=NULL,state='queued',runtime_state='offline',request_config='{}' WHERE workspace_id='ws-bob'")
            .bind(&workspace).execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO zuse_rust_control_owners VALUES('workspace',$1,'rust',1,$2)")
            .bind(&workspace)
            .bind(now)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO api_cloud_workspace_launch_intents VALUES($1,'user-bob','chat','session','turn','command','test-ciphertext',$2,$3)")
            .bind(&workspace).bind(now+86_400_000).bind(now).execute(&pool).await.unwrap();
        let signing = ed25519_dalek::SigningKey::from_bytes(&[77; 32]);
        let jwk = json!({"kty":"OKP","crv":"Ed25519","d":URL_SAFE_NO_PAD.encode([77;32]),"x":URL_SAFE_NO_PAD.encode(signing.verifying_key().as_bytes())});
        let control = RuntimeControl::from_pool(
            pool.clone(),
            MasterKey::parse(&URL_SAFE_NO_PAD.encode([78; 32])).unwrap(),
            TicketSigner::from_jwk(&jwk.to_string(), "https://api.example.test").unwrap(),
        );
        let operation = control
            .queue_allocation(&workspace, "managed-artifact", now)
            .await
            .unwrap();
        if inject_at == "readiness" {
            // Exercise the prepared fence directly: enrollment arrives after
            // the earlier allocation snapshot, while command readiness runs.
            sqlx::query("UPDATE api_cloud_workspaces SET request_config=request_config||$2 WHERE workspace_id=$1")
                .bind(&workspace).bind(json!({"rustFastStartVersion":1,"agent":"codex","rustProviderControllerVersion":1,"managedReleaseId":"prepared-test","initialRuntimeBootstrap":{"version":1,"mode":"stopped-memory-snapshot-v1","runtimeSha256":"a".repeat(64),"supervisorSha256":"b".repeat(64),"serviceSha256":"c".repeat(64),"manifestSha256":"d".repeat(64),"helperSha256":"e".repeat(64)}}))
                .execute(&pool).await.unwrap();
        }
        let disk: String = sqlx::query_scalar("UPDATE zuse_rust_allocations SET phase='launching',machine_id='original' WHERE operation_id=$1 RETURNING storage_incarnation_id")
            .bind(&operation).fetch_one(&pool).await.unwrap();
        sqlx::query(
            "UPDATE api_cloud_workspaces SET provider_sandbox_id='original' WHERE workspace_id=$1",
        )
        .bind(&workspace)
        .execute(&pool)
        .await
        .unwrap();
        let fixture = Preparing {
            pool: pool.clone(),
            workspace: workspace.clone(),
            config: enrolled(&workspace, &operation, &disk, "failed"),
            inject_at,
            inspections: AtomicUsize::new(0),
            readiness: AtomicUsize::new(0),
            launches: AtomicUsize::new(0),
        };
        if inject_at == "existing" {
            fixture.inject().await;
        }
        if !fault.is_empty() {
            let job = jobs::claim(&pool, JOB_KIND, "workspace", clock_ms(), 120_000)
                .await
                .unwrap()
                .unwrap();
            let statement = match fault {
                "expired-lease" => {
                    "UPDATE zuse_rust_jobs SET lease_expires_at=0 WHERE resource_id=$1"
                }
                "replaced-lease" => {
                    "UPDATE zuse_rust_jobs SET lease_token='replacement-lease' WHERE resource_id=$1"
                }
                "delete" => {
                    "UPDATE api_cloud_workspaces SET desired_state='deleted' WHERE workspace_id=$1"
                }
                "generation" => {
                    "UPDATE zuse_rust_control_owners SET generation=generation+1 WHERE resource_id=$1"
                }
                _ => unreachable!(),
            };
            sqlx::query(statement)
                .bind(&workspace)
                .execute(&pool)
                .await
                .unwrap();
            // Capture after the competing actor changed authority. A stale hold
            // must leave that actor's lease, deletion and retry budget untouched.
            let before: Value =
                sqlx::query_scalar("SELECT to_jsonb(j) FROM zuse_rust_jobs j WHERE job_id=$1")
                    .bind(&job.id)
                    .fetch_one(&pool)
                    .await
                    .unwrap();
            let result = control.hold_preparing_allocation(&job).await;
            assert!(matches!(result, Ok(false) | Err(_)), "{fault}");
            let after: Value =
                sqlx::query_scalar("SELECT to_jsonb(j) FROM zuse_rust_jobs j WHERE job_id=$1")
                    .bind(&job.id)
                    .fetch_one(&pool)
                    .await
                    .unwrap();
            assert_eq!(before, after, "{fault}");
            assert_eq!(fixture.launches.load(Ordering::SeqCst), 0);
            drop(control);
            drop(fixture);
            pool.close().await;
            sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
                .execute(&admin)
                .await
                .unwrap();
            continue;
        }
        for _ in 0..3 {
            sqlx::query("UPDATE zuse_rust_jobs SET due_at=0 WHERE resource_id=$1")
                .bind(&workspace)
                .execute(&pool)
                .await
                .unwrap();
            assert!(control.allocate_once(&fixture, clock_ms()).await.unwrap());
            let state: (String, String, i32, String, Option<String>) = sqlx::query_as("SELECT w.status_code,j.state,j.attempts,a.phase,j.last_error_code FROM api_cloud_workspaces w JOIN zuse_rust_jobs j ON j.resource_id=w.workspace_id JOIN zuse_rust_allocations a ON a.workspace_id=w.workspace_id WHERE w.workspace_id=$1")
                .bind(&workspace).fetch_one(&pool).await.unwrap();
            assert_eq!(
                state,
                (
                    "repository_preparation_failed".into(),
                    "pending".into(),
                    0,
                    "launching".into(),
                    None
                ),
                "{inject_at}"
            );
        }
        assert_eq!(
            fixture.inspections.load(Ordering::SeqCst),
            usize::from(inject_at != "existing")
        );
        assert_eq!(fixture.launches.load(Ordering::SeqCst), 0);
        // A real ready receipt can complete the same allocation immediately;
        // preparation holds never gate guest/command admission or replay launch.
        sqlx::query("UPDATE api_cloud_workspaces SET state='ready',runtime_state='online',status_code='ready',request_config=jsonb_set(request_config,'{rustCompletedLaunchCommandId}','\"command\"') WHERE workspace_id=$1")
            .bind(&workspace).execute(&pool).await.unwrap();
        sqlx::query("UPDATE zuse_rust_jobs SET due_at=0 WHERE resource_id=$1")
            .bind(&workspace)
            .execute(&pool)
            .await
            .unwrap();
        assert!(control.allocate_once(&fixture, clock_ms()).await.unwrap());
        let done: (String, String) = sqlx::query_as("SELECT a.phase,j.state FROM zuse_rust_allocations a JOIN zuse_rust_jobs j ON j.resource_id=a.workspace_id WHERE a.workspace_id=$1")
            .bind(&workspace).fetch_one(&pool).await.unwrap();
        assert_eq!(done, ("ready".into(), "completed".into()));
        drop(control);
        drop(fixture);
        pool.close().await;
        sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
            .execute(&admin)
            .await
            .unwrap();
    }
    admin.close().await;
}
