use super::*;
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};

pub(super) struct Remote {
    pub(super) pool: PgPool,
    pub(super) workspace: String,
    pub(super) machine: Mutex<MachineState>,
    pub(super) calls: AtomicUsize,
    pub(super) observations: Mutex<Vec<Option<String>>>,
    pub(super) complete: AtomicBool,
    pub(super) deleted: AtomicBool,
    pub(super) expire: bool,
    pub(super) uncertain: bool,
}
impl Remote {
    pub(super) async fn assert_unlocked(&self) {
        tokio::time::timeout(
            Duration::from_secs(2),
            sqlx::query(
                "UPDATE api_cloud_workspaces SET revision=revision+1 WHERE workspace_id=$1",
            )
            .bind(&self.workspace)
            .execute(&self.pool),
        )
        .await
        .expect("Provider I/O must not hold the workspace row lock")
        .unwrap();
    }
}
#[async_trait]
impl RetainedProvider for Remote {
    fn provider_id(&self) -> &'static str {
        "boat"
    }
    async fn inspect_retained(&self, id: &str) -> Result<Option<Machine>, ProviderError> {
        self.assert_unlocked().await;
        if self.deleted.load(Ordering::SeqCst) {
            return Ok(None);
        }
        Ok(Some(Machine {
            id: id.into(),
            name: "fixture".into(),
            state: self.machine.lock().unwrap().clone(),
            vcpu: None,
            memory_bytes: None,
        }))
    }
    async fn pause_retained(&self, _: &str) -> Result<(), ProviderError> {
        panic!("The receipt-bearing path is required")
    }
    async fn wake_retained(&self, _: &str, _: &MachineState) -> Result<(), ProviderError> {
        panic!("Sleep test")
    }
    async fn launch_retained(&self, _: &Directive, _: &str, _: &str) -> Result<(), ProviderError> {
        panic!("Sleep test")
    }
    async fn sleep_receipt(&self, id: &str) -> Result<ProviderOperation, ProviderError> {
        self.assert_unlocked().await;
        self.calls.fetch_add(1, Ordering::SeqCst);
        *self.machine.lock().unwrap() = MachineState::Stopped;
        if self.expire {
            sqlx::query("UPDATE zuse_rust_jobs SET lease_expires_at=0 WHERE resource_id=$1 AND state='running'").bind(&self.workspace).execute(&self.pool).await.unwrap();
        }
        if self.uncertain {
            return Err(ProviderError::OutcomeUnknown);
        }
        Ok(ProviderOperation {
            resource_id: id.into(),
            operation_id: Some("original-stop-id".into()),
            desired: DesiredState::Sleeping,
        })
    }
    async fn receipt_state(&self, r: &ProviderOperation) -> Result<OperationState, ProviderError> {
        self.assert_unlocked().await;
        self.observations
            .lock()
            .unwrap()
            .push(r.operation_id.clone());
        if r.operation_id.as_deref() != Some("original-stop-id") {
            return Err(ProviderError::InvalidResponse);
        }
        Ok(if self.complete.load(Ordering::SeqCst) {
            OperationState::Completed
        } else {
            OperationState::Retrying
        })
    }
}
#[async_trait]
impl crate::runtime::deletion::DeletionProvider for Remote {
    fn provider_id(&self) -> &'static str {
        "boat"
    }
    async fn find(&self, id: &str) -> Result<Option<Machine>, ProviderError> {
        self.inspect_retained(id).await
    }
    async fn destroy(&self, _: &str) -> Result<(), ProviderError> {
        self.assert_unlocked().await;
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.deleted.store(true, Ordering::SeqCst);
        if self.expire {
            sqlx::query("UPDATE zuse_rust_jobs SET lease_expires_at=0 WHERE resource_id=$1 AND kind='workspace-delete'").bind(&self.workspace).execute(&self.pool).await.unwrap();
        }
        if self.uncertain {
            Err(ProviderError::OutcomeUnknown)
        } else {
            Ok(())
        }
    }
}
struct DeleteObjects(Arc<Remote>);
#[async_trait]
impl crate::objects::Objects for DeleteObjects {
    fn supports_purge(&self) -> bool {
        true
    }
    async fn get(&self, _: &str) -> Result<Option<Vec<u8>>, ApiError> {
        panic!("Deletion must not read chat history")
    }
    async fn put(&self, _: &str, _: &[u8]) -> Result<(), ApiError> {
        panic!("Deletion must not upload data")
    }
    async fn purge_prefix(&self, _: &str) -> Result<bool, ApiError> {
        self.0.assert_unlocked().await;
        assert!(
            self.0.deleted.load(Ordering::SeqCst),
            "Object deletion must wait for resource absence"
        );
        Ok(true)
    }
}
pub(super) fn control(pool: PgPool) -> RuntimeControl {
    let signing = ed25519_dalek::SigningKey::from_bytes(&[71; 32]);
    let jwk = json!({"kty":"OKP","crv":"Ed25519","d":URL_SAFE_NO_PAD.encode([71;32]),"x":URL_SAFE_NO_PAD.encode(signing.verifying_key().as_bytes())});
    RuntimeControl::from_pool(
        pool,
        MasterKey::parse(&URL_SAFE_NO_PAD.encode([72; 32])).unwrap(),
        TicketSigner::from_jwk(&jwk.to_string(), "https://api.example.test").unwrap(),
    )
}

#[tokio::test]
#[ignore = "requires explicit isolated localhost PostgreSQL"]
async fn original_stop_receipt_survives_worker_replacement_lease_loss_and_pending_archival() {
    let url = std::env::var("ZUSE_CLOUD_TEST_DATABASE_URL").expect("isolated PostgreSQL required");
    let url_check = reqwest::Url::parse(&url).unwrap();
    assert!(matches!(
        url_check.host_str(),
        Some("localhost" | "127.0.0.1" | "::1")
    ));
    let admin = PgPool::connect(&url).await.unwrap();
    for (expire, uncertain, already_stopped) in [
        (false, false, false),
        (true, false, false),
        (false, true, false),
        (false, false, true),
    ] {
        let expected_calls = usize::from(!already_stopped);
        let unique = uuid::Uuid::new_v4().simple().to_string();
        let schema = format!("lifecycle_{unique}");
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
            include_str!("../../../../migrations/0012_provider_receipts.sql"),
            include_str!("../../../../migrations/0008_delete.sql"),
        ] {
            sqlx::raw_sql(sql).execute(&pool).await.unwrap();
        }
        let workspace = format!("ws-{unique}");
        let operation = format!("op-{unique}");
        let config = json!({"runtimeGeneration":2,"gatewayEpoch":2,"runtimeRpcProtocol":zuse_protocol::cloud_gateway::RUST_RPC,"rustRuntimeStorage":{"storageIncarnationId":"original-disk"},"rustLifecycleOperation":{"id":operation,"action":"pause","phase":"quiesced"}});
        sqlx::query("UPDATE api_cloud_workspaces SET workspace_id=$1,provider='boat',provider_sandbox_id='machine-original',state='pausing',desired_state='paused',request_config=$2 WHERE workspace_id='ws-bob'").bind(&workspace).bind(config).execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO zuse_rust_control_owners VALUES('workspace',$1,'rust',2,0)")
            .bind(&workspace)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO zuse_rust_lifecycle_operations(operation_id,workspace_id,command_id,generation,source_machine_id,storage_incarnation_id,action,phase,created_at,updated_at) VALUES($1,$2,'command',2,'machine-original','original-disk','pause','quiesced',$3,$3)").bind(&operation).bind(&workspace).bind(clock_ms()).execute(&pool).await.unwrap();
        let mut tx = pool.begin().await.unwrap();
        jobs::enqueue(
            &mut tx,
            &operation,
            JOB_KIND,
            &workspace,
            2,
            &json!({"operationId":operation}),
            clock_ms(),
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
        let remote = Arc::new(Remote {
            pool: pool.clone(),
            workspace: workspace.clone(),
            machine: Mutex::new(if already_stopped {
                MachineState::Stopped
            } else {
                MachineState::Running
            }),
            calls: AtomicUsize::new(0),
            observations: Mutex::new(vec![]),
            complete: AtomicBool::new(false),
            deleted: AtomicBool::new(false),
            expire,
            uncertain,
        });
        let first = control(pool.clone())
            .lifecycle_once(remote.as_ref(), clock_ms())
            .await;
        assert_eq!(first.is_err(), expire || uncertain || already_stopped);
        assert_eq!(remote.calls.load(Ordering::SeqCst), expected_calls);
        let stored: Value =
            sqlx::query_scalar("SELECT provider_effects FROM zuse_rust_lifecycle_operations")
                .fetch_one(&pool)
                .await
                .unwrap();
        if already_stopped {
            assert!(stored["sleep"].is_null());
        } else {
            assert_eq!(
                stored["sleep"]["state"],
                if uncertain { "uncertain" } else { "accepted" }
            );
        }
        if !uncertain && !already_stopped {
            assert_eq!(
                stored["sleep"]["receipt"]["operationId"],
                "original-stop-id"
            );
        }
        // New controller instance has no in-process knowledge of the submission.
        let restarted = control(pool.clone());
        for _ in 0..3 {
            sqlx::query("UPDATE zuse_rust_jobs SET due_at=0 WHERE resource_id=$1")
                .bind(&workspace)
                .execute(&pool)
                .await
                .unwrap();
            let observed = restarted.lifecycle_once(remote.as_ref(), clock_ms()).await;
            assert_eq!(observed.is_err(), uncertain || already_stopped);
            assert_eq!(
                remote.calls.load(Ordering::SeqCst),
                expected_calls,
                "A stopped-looking resource is not permission to submit another Stop"
            );
            let phase: String =
                sqlx::query_scalar("SELECT phase FROM zuse_rust_lifecycle_operations")
                    .fetch_one(&pool)
                    .await
                    .unwrap();
            assert_ne!(
                phase, "paused",
                "An archived machine does not settle a pending or unknown Stop"
            );
        }
        if !uncertain && !already_stopped {
            assert!(
                remote
                    .observations
                    .lock()
                    .unwrap()
                    .iter()
                    .all(|id| id.as_deref() == Some("original-stop-id"))
            );
            remote.complete.store(true, Ordering::SeqCst);
            sqlx::query("UPDATE zuse_rust_jobs SET due_at=0 WHERE resource_id=$1")
                .bind(&workspace)
                .execute(&pool)
                .await
                .unwrap();
            restarted
                .lifecycle_once(remote.as_ref(), clock_ms())
                .await
                .unwrap();
            let state: String =
                sqlx::query_scalar("SELECT state FROM api_cloud_workspaces WHERE workspace_id=$1")
                    .bind(&workspace)
                    .fetch_one(&pool)
                    .await
                    .unwrap();
            assert_eq!(state, "paused");
            assert_eq!(remote.calls.load(Ordering::SeqCst), 1);
        }
        // Deletion uses the same short-transaction policy and original target
        // receipt, including an accepted delete whose response/lease was lost.
        remote.calls.store(0, Ordering::SeqCst);
        let delete_id = format!("delete-{unique}");
        sqlx::query("UPDATE api_cloud_workspaces SET state='deleting',desired_state='deleted' WHERE workspace_id=$1").bind(&workspace).execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO zuse_rust_deletions(workspace_id,account_id,operation_id,command_digest,generation,phase,targets,requested_at) VALUES($1,'user-bob',$2,'fixture',2,'providers',$3,$4)")
            .bind(&workspace).bind(&delete_id).bind(json!([{"id":"machine-original","name":null,"uncertain":false,"done":false}])).bind(clock_ms()).execute(&pool).await.unwrap();
        let mut tx = pool.begin().await.unwrap();
        jobs::enqueue(
            &mut tx,
            &delete_id,
            "workspace-delete",
            &workspace,
            2,
            &json!({"operationId":delete_id}),
            clock_ms(),
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
        let cleanup =
            control(pool.clone()).with_transcript_objects(Arc::new(DeleteObjects(remote.clone())));
        for _ in 0..8 {
            sqlx::query("UPDATE zuse_rust_jobs SET due_at=0 WHERE resource_id=$1 AND kind='workspace-delete'").bind(&workspace).execute(&pool).await.unwrap();
            let _ = cleanup.delete_once(remote.as_ref(), clock_ms()).await;
            let state: String =
                sqlx::query_scalar("SELECT state FROM api_cloud_workspaces WHERE workspace_id=$1")
                    .bind(&workspace)
                    .fetch_one(&pool)
                    .await
                    .unwrap();
            if state == "deleted" {
                break;
            }
        }
        let state: String =
            sqlx::query_scalar("SELECT state FROM api_cloud_workspaces WHERE workspace_id=$1")
                .bind(&workspace)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(state, "deleted");
        assert_eq!(
            remote.calls.load(Ordering::SeqCst),
            1,
            "Lost delete responses must not dispatch another deletion"
        );
        drop(cleanup);
        drop(remote);
        drop(restarted);
        pool.close().await;
        sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
            .execute(&admin)
            .await
            .unwrap();
    }
}

struct MemoryRemote(Remote);
#[async_trait]
impl RetainedProvider for MemoryRemote {
    async fn inspect_retained(&self, id: &str) -> Result<Option<Machine>, ProviderError> {
        self.0.inspect_retained(id).await
    }
    async fn pause_retained(&self, _: &str) -> Result<(), ProviderError> {
        panic!("a parked guest cannot use cold shutdown")
    }
    async fn wake_retained(&self, _: &str, _: &MachineState) -> Result<(), ProviderError> {
        panic!("sleep only")
    }
    async fn launch_retained(&self, _: &Directive, _: &str, _: &str) -> Result<(), ProviderError> {
        panic!("sleep only")
    }
    async fn sleep_receipt(&self, _: &str) -> Result<ProviderOperation, ProviderError> {
        panic!("memory receipt must choose memory suspend")
    }
    async fn memory_sleep_receipt(&self, id: &str) -> Result<ProviderOperation, ProviderError> {
        self.0.assert_unlocked().await;
        self.0.calls.fetch_add(1, Ordering::SeqCst);
        *self.0.machine.lock().unwrap() = MachineState::Standby;
        if self.0.expire {
            sqlx::query("UPDATE zuse_rust_jobs SET lease_expires_at=0 WHERE resource_id=$1")
                .bind(&self.0.workspace)
                .execute(&self.0.pool)
                .await
                .unwrap();
        }
        if self.0.uncertain {
            return Err(ProviderError::OutcomeUnknown);
        }
        Ok(ProviderOperation {
            resource_id: id.into(),
            operation_id: Some("original-memory-suspend".into()),
            desired: DesiredState::Sleeping,
        })
    }
    async fn receipt_state(&self, _: &ProviderOperation) -> Result<OperationState, ProviderError> {
        panic!("cold observation cannot certify RAM")
    }
    async fn memory_receipt_state(
        &self,
        r: &ProviderOperation,
    ) -> Result<OperationState, ProviderError> {
        self.0.assert_unlocked().await;
        self.0
            .observations
            .lock()
            .unwrap()
            .push(r.operation_id.clone());
        assert_eq!(r.resource_id, "memory-source");
        assert_eq!(r.desired, DesiredState::Sleeping);
        if r.operation_id.as_deref() != Some("original-memory-suspend") {
            return Err(ProviderError::InvalidResponse);
        }
        Ok(if self.0.complete.load(Ordering::SeqCst) {
            OperationState::Completed
        } else {
            OperationState::Retrying
        })
    }
}
#[tokio::test]
#[ignore = "requires explicit isolated localhost PostgreSQL"]
async fn memory_suspend_requires_bound_park_and_reconciles_the_original_operation() {
    let url = std::env::var("ZUSE_CLOUD_TEST_DATABASE_URL").unwrap();
    assert!(matches!(
        reqwest::Url::parse(&url).unwrap().host_str(),
        Some("localhost" | "127.0.0.1" | "::1")
    ));
    let admin = PgPool::connect(&url).await.unwrap();
    for (expire, uncertain) in [(false, false), (true, false), (true, true)] {
        let schema = format!("memory_{}", uuid::Uuid::new_v4().simple());
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
            include_str!("../../../../tests/fixtures/billing.sql"),
            jobs::MIGRATION,
            MIGRATION,
            include_str!("../../../../migrations/0012_provider_receipts.sql"),
        ] {
            sqlx::raw_sql(sql).execute(&pool).await.unwrap();
        }
        let workspace = format!("memory-ws-{}", uuid::Uuid::new_v4().simple());
        let operation = "memory-operation";
        let config = json!({"runtimeGeneration":2,"gatewayEpoch":2,"runtimeRpcProtocol":zuse_protocol::cloud_gateway::RUST_RPC,"runtimeBootstrapReceipt":{"workspaceId":workspace,"generation":2,"gatewayEpoch":2},"runtimeCredentialExpiresAtMs":9007199254740991_i64,"runtimeCredentialKeyThumbprint":"credential-key","runtimeSigningKeyThumbprint":"signing-key","rustRuntimeStorage":{"storageIncarnationId":"memory-disk"},"rustMemorySleepVersion":1,"rustStorageEnrollment":{"phase":"committed","generation":2,"sourceSandboxId":"memory-source","storageIncarnationId":"memory-disk","credentialKeyThumbprint":"credential-key","signingKeyThumbprint":"signing-key","capabilities":[zuse_protocol::cloud_lifecycle::CAPABILITY,zuse_protocol::cloud_lifecycle::MEMORY_CAPABILITY]},"rustLifecycleOperation":{"id":operation,"action":"pause","phase":"quiescing"}});
        sqlx::query("UPDATE api_cloud_workspaces SET workspace_id=$1,provider='boxd',provider_sandbox_id='memory-source',state='pausing',desired_state='paused',runtime_credential_hash=$2,request_config=$3 WHERE workspace_id='ws-bob'").bind(&workspace).bind(hash("memory-credential")).bind(&config).execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO zuse_rust_control_owners VALUES('workspace',$1,'rust',2,0)")
            .bind(&workspace)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO zuse_rust_lifecycle_operations(operation_id,workspace_id,command_id,generation,source_machine_id,storage_incarnation_id,action,phase,created_at,updated_at) VALUES($1,$2,'pause-command',2,'memory-source','memory-disk','pause','quiescing',$3,$3)").bind(operation).bind(&workspace).bind(clock_ms()).execute(&pool).await.unwrap();
        let control = control(pool.clone());
        for bad in [
            "version",
            "capability",
            "provider",
            "operation",
            "legacy-quiesced",
        ] {
            let mut wrong = config.clone();
            if bad == "version" {
                wrong["rustMemorySleepVersion"] = json!(0);
            }
            if bad == "capability" {
                wrong["rustStorageEnrollment"]["capabilities"] = json!([]);
            }
            sqlx::query("UPDATE api_cloud_workspaces SET request_config=$2,provider=$3 WHERE workspace_id=$1").bind(&workspace).bind(&wrong).bind(if bad=="provider"{"boat"}else{"boxd"}).execute(&pool).await.unwrap();
            sqlx::query("UPDATE zuse_rust_lifecycle_operations SET phase=$1")
                .bind(if bad == "legacy-quiesced" {
                    "quiesced"
                } else {
                    "quiescing"
                })
                .execute(&pool)
                .await
                .unwrap();
            assert!(
                control
                    .lifecycle_runtime(
                        &workspace,
                        "memory-credential",
                        RuntimeRequest::Parked {
                            operation_id: if bad == "operation" {
                                "wrong-operation"
                            } else {
                                operation
                            }
                            .into()
                        },
                        clock_ms()
                    )
                    .await
                    .is_err()
            );
        }
        sqlx::query("UPDATE api_cloud_workspaces SET request_config=$2,provider='boxd' WHERE workspace_id=$1").bind(&workspace).bind(&config).execute(&pool).await.unwrap();
        sqlx::query("UPDATE zuse_rust_lifecycle_operations SET phase='quiescing'")
            .execute(&pool)
            .await
            .unwrap();
        for _ in 0..2 {
            control
                .lifecycle_runtime(
                    &workspace,
                    "memory-credential",
                    RuntimeRequest::Parked {
                        operation_id: operation.into(),
                    },
                    clock_ms(),
                )
                .await
                .unwrap();
        }
        let marker:Value=sqlx::query_scalar("SELECT request_config->'rustMemoryParked' FROM api_cloud_workspaces WHERE workspace_id=$1").bind(&workspace).fetch_one(&pool).await.unwrap();
        assert_eq!(
            marker,
            json!({"version":1,"operationId":operation,"sourceMachineId":"memory-source","generation":2,"storageIncarnationId":"memory-disk"})
        );
        let mut tx = pool.begin().await.unwrap();
        jobs::enqueue(
            &mut tx,
            operation,
            JOB_KIND,
            &workspace,
            2,
            &json!({"operationId":operation}),
            clock_ms(),
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
        sqlx::query("UPDATE zuse_rust_jobs SET max_attempts=120 WHERE resource_id=$1")
            .bind(&workspace)
            .execute(&pool)
            .await
            .unwrap();
        let remote = MemoryRemote(Remote {
            pool: pool.clone(),
            workspace: workspace.clone(),
            machine: Mutex::new(MachineState::Running),
            calls: AtomicUsize::new(0),
            observations: Mutex::new(vec![]),
            complete: AtomicBool::new(false),
            deleted: AtomicBool::new(false),
            expire,
            uncertain,
        });
        if !expire && !uncertain {
            let held = jobs::claim(&pool, JOB_KIND, "workspace", clock_ms(), 120000)
                .await
                .unwrap()
                .unwrap();
            let mut blocker = pool.begin().await.unwrap();
            sqlx::query(
                "SELECT workspace_id FROM api_cloud_workspaces WHERE workspace_id=$1 FOR UPDATE",
            )
            .bind(&workspace)
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
            sqlx::query("UPDATE zuse_rust_jobs SET lease_expires_at=$2 WHERE job_id=$1")
                .bind(&held.id)
                .bind(clock_ms() + 50)
                .execute(&mut *blocker)
                .await
                .unwrap();
            let secondary = self::control(pool.clone());
            let waiting_job = held.clone();
            let waiting =
                tokio::spawn(async move { secondary.lifecycle_snapshot(&waiting_job).await });
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            blocker.commit().await.unwrap();
            assert_eq!(
                tokio::time::timeout(std::time::Duration::from_secs(2), waiting)
                    .await
                    .unwrap()
                    .unwrap()
                    .unwrap_err()
                    .code,
                "cloud_lifecycle_fenced",
                "lease expiry while waiting on workspace lock cannot authorize a provider effect"
            );
            sqlx::query("UPDATE zuse_rust_jobs SET state='pending',lease_token=NULL,lease_expires_at=NULL,due_at=0 WHERE job_id=$1").bind(&held.id).execute(&pool).await.unwrap();
            // The guest ACK can arrive after a worker observed quiescing but
            // before that delivery saves. It must remain immediately due.
            let job = jobs::claim(&pool, JOB_KIND, "workspace", clock_ms(), 120000)
                .await
                .unwrap()
                .unwrap();
            control
                .lifecycle_save(&job, "quiescing", None, false, false)
                .await
                .unwrap();
            let due: i64 =
                sqlx::query_scalar("SELECT due_at FROM zuse_rust_jobs WHERE resource_id=$1")
                    .bind(&workspace)
                    .fetch_one(&pool)
                    .await
                    .unwrap();
            assert!(
                due <= clock_ms(),
                "ACK race cannot restore a two-second delay"
            );
        }
        if !expire && !uncertain {
            // The production fast path drains immediate progress without
            // unrelated worker scans or a timer, even when hints are dropped.
            let burst = crate::runtime::lifecycle::scheduling::ready_burst(|| {
                control.lifecycle_once(&remote, clock_ms())
            })
            .await;
            assert!(burst.error.is_none());
            assert_eq!(
                burst.claimed, 2,
                "new receipt followed by one original-operation observation"
            );
            assert!(!burst.limit_reached);
            eprintln!(
                "isolated_pg_lifecycle_burst_ms={}",
                burst.elapsed.as_millis()
            );
            let due: i64 =
                sqlx::query_scalar("SELECT due_at FROM zuse_rust_jobs WHERE resource_id=$1")
                    .bind(&workspace)
                    .fetch_one(&pool)
                    .await
                    .unwrap();
            assert!(
                due >= clock_ms() + 1500,
                "pending original receipt keeps bounded two-second reconciliation"
            );
            assert!(
                !control.lifecycle_once(&remote, clock_ms()).await.unwrap(),
                "no busy polling while provider is pending"
            );
            assert_eq!(remote.0.calls.load(Ordering::SeqCst), 1);
        } else {
            assert!(control.lifecycle_once(&remote, clock_ms()).await.is_err());
        }
        for _ in 0..3 {
            sqlx::query("UPDATE zuse_rust_jobs SET due_at=0 WHERE resource_id=$1")
                .bind(&workspace)
                .execute(&pool)
                .await
                .unwrap();
            assert_eq!(
                control.lifecycle_once(&remote, clock_ms()).await.is_err(),
                uncertain
            );
            assert_eq!(
                remote.0.calls.load(Ordering::SeqCst),
                1,
                "no second Suspend after a lost reply or lease"
            );
            let phase: String =
                sqlx::query_scalar("SELECT phase FROM zuse_rust_lifecycle_operations")
                    .fetch_one(&pool)
                    .await
                    .unwrap();
            assert_ne!(phase, "paused");
        }
        if !uncertain {
            remote.0.complete.store(true, Ordering::SeqCst);
            sqlx::query("UPDATE zuse_rust_jobs SET due_at=0 WHERE resource_id=$1")
                .bind(&workspace)
                .execute(&pool)
                .await
                .unwrap();
            control.lifecycle_once(&remote, clock_ms()).await.unwrap();
            let phase: String =
                sqlx::query_scalar("SELECT phase FROM zuse_rust_lifecycle_operations")
                    .fetch_one(&pool)
                    .await
                    .unwrap();
            assert_eq!(phase, "paused");
        }
        if !expire && !uncertain {
            for leased in [false, true] {
                let resume = uuid::Uuid::new_v4().to_string();
                let at = clock_ms();
                sqlx::query("INSERT INTO zuse_rust_lifecycle_operations(operation_id,workspace_id,command_id,generation,source_machine_id,storage_incarnation_id,action,phase,created_at,updated_at) VALUES($1,$2,$1,2,'memory-source','memory-disk','resume','launching',$3,$3)").bind(&resume).bind(&workspace).bind(at).execute(&pool).await.unwrap();
                sqlx::query("UPDATE api_cloud_workspaces SET state='provisioning',desired_state='ready',runtime_state='connecting',request_config=jsonb_set(request_config,'{rustLifecycleOperation}',$2) WHERE workspace_id=$1").bind(&workspace).bind(json!({"id":resume,"action":"resume","phase":"launching"})).execute(&pool).await.unwrap();
                let mut tx = pool.begin().await.unwrap();
                jobs::enqueue(
                    &mut tx,
                    &format!("ack-{resume}"),
                    JOB_KIND,
                    &workspace,
                    2,
                    &json!({"operationId":resume}),
                    at,
                )
                .await
                .unwrap();
                tx.commit().await.unwrap();
                let old = if leased {
                    Some(
                        jobs::claim(&pool, JOB_KIND, "workspace", at + 1, 120000)
                            .await
                            .unwrap()
                            .unwrap(),
                    )
                } else {
                    None
                };
                if let Some(job) = old.as_ref() {
                    // Model the retained pause receipt for this independent resume.
                    sqlx::query("UPDATE api_cloud_workspaces SET request_config=jsonb_set(request_config,'{rustMemoryParked}',$2) WHERE workspace_id=$1")
                        .bind(&workspace).bind(json!({"version":1,"generation":2,"operationId":operation,"sourceMachineId":"memory-source","storageIncarnationId":"memory-disk"})).execute(&pool).await.unwrap();
                    let mut retained = control
                        .personal_memory_connection(job)
                        .await
                        .unwrap()
                        .expect("personal bound RAM resume reuses its checkout");
                    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
                        .fetch_one(&mut *retained)
                        .await
                        .unwrap();
                    let (tx, _, _, _) = control
                        .lifecycle_locked_with(Some(&mut *retained), job)
                        .await
                        .unwrap();
                    tx.commit().await.unwrap();
                    assert_eq!(
                        pid,
                        sqlx::query_scalar::<_, i32>("SELECT pg_backend_pid()")
                            .fetch_one(&mut *retained)
                            .await
                            .unwrap(),
                        "adjacent admissions must use the same physical connection"
                    );
                    sqlx::query("UPDATE zuse_rust_jobs SET lease_expires_at=0 WHERE job_id=$1")
                        .bind(&job.id)
                        .execute(&pool)
                        .await
                        .unwrap();
                    assert_eq!(
                        control
                            .lifecycle_locked_with(Some(&mut *retained), job)
                            .await
                            .err()
                            .unwrap()
                            .code,
                        "cloud_lifecycle_fenced",
                        "a cached connection cannot retain an expired lease"
                    );
                    // Flush the queued dropped-transaction rollback before another
                    // connection takes these locks. Production drops its checkout.
                    sqlx::query("SELECT 1")
                        .execute(&mut *retained)
                        .await
                        .unwrap();
                    sqlx::query("UPDATE zuse_rust_jobs SET lease_expires_at=$2 WHERE job_id=$1")
                        .bind(&job.id)
                        .bind(clock_ms() + 120000)
                        .execute(&pool)
                        .await
                        .unwrap();
                    sqlx::query(
                        "UPDATE zuse_rust_control_owners SET generation=3 WHERE resource_id=$1",
                    )
                    .bind(&workspace)
                    .execute(&pool)
                    .await
                    .unwrap();
                    assert_eq!(
                        control
                            .lifecycle_locked_with(Some(&mut *retained), job)
                            .await
                            .err()
                            .unwrap()
                            .code,
                        "workspace_runtime_fenced",
                        "a cached connection cannot retain withdrawn generation authority"
                    );
                    // Flush the queued dropped-transaction rollback before another
                    // connection takes these locks. Production drops its checkout.
                    sqlx::query("SELECT 1")
                        .execute(&mut *retained)
                        .await
                        .unwrap();
                    sqlx::query(
                        "UPDATE zuse_rust_control_owners SET generation=2 WHERE resource_id=$1",
                    )
                    .bind(&workspace)
                    .execute(&pool)
                    .await
                    .unwrap();
                    drop(retained);
                }
                assert!(
                    control
                        .lifecycle_runtime(
                            &workspace,
                            "memory-credential",
                            RuntimeRequest::Resumed {
                                operation_id: "wrong-original-operation".into()
                            },
                            at + 2
                        )
                        .await
                        .is_err()
                );
                let start = std::time::Instant::now();
                control
                    .lifecycle_runtime(
                        &workspace,
                        "memory-credential",
                        RuntimeRequest::Resumed {
                            operation_id: resume.clone(),
                        },
                        at + 3,
                    )
                    .await
                    .unwrap();
                eprintln!("isolated_pg_resumed_ack_ms={}", start.elapsed().as_millis());
                let (state,lease,finished):(String,Option<String>,i64)=sqlx::query_as("SELECT state,lease_token,updated_at FROM zuse_rust_jobs WHERE payload->>'operationId'=$1").bind(&resume).fetch_one(&pool).await.unwrap();
                assert_eq!(state, "completed");
                assert_eq!(lease, None);
                assert_eq!(
                    finished,
                    at + 3,
                    "actual Ready ACK completes matching durable job, no worker/timer needed"
                );
                if let Some(old) = old {
                    assert_eq!(
                        control
                            .lifecycle_save(&old, "launching", None, false, false)
                            .await
                            .unwrap_err()
                            .code,
                        "cloud_lifecycle_fenced"
                    );
                    assert!(
                        !jobs::retry(&pool, &old, at + 4, "cloud_lifecycle_fenced")
                            .await
                            .unwrap(),
                        "late lease cannot reopen completed resume"
                    );
                }
                assert!(
                    !control.lifecycle_once(&remote, clock_ms()).await.unwrap(),
                    "no final provider observation/relaunch after real Ready"
                );
                control
                    .lifecycle_runtime(
                        &workspace,
                        "memory-credential",
                        RuntimeRequest::Resumed {
                            operation_id: resume.clone(),
                        },
                        at + 500,
                    )
                    .await
                    .unwrap();
                let (ready,done):(i64,i64)=sqlx::query_as("SELECT (o.provider_effects->'timings'->>'ready')::bigint,j.updated_at FROM zuse_rust_lifecycle_operations o JOIN zuse_rust_jobs j ON j.payload->>'operationId'=o.operation_id WHERE o.operation_id=$1").bind(&resume).fetch_one(&pool).await.unwrap();
                assert_eq!(
                    (ready, done),
                    (at + 3, at + 3),
                    "duplicate ACK preserves first readiness/completion timings"
                );
            }
            // Exercise the actual request admission (not a hand-written intent):
            // durable operation, updated public row and exactly one pending job.
            let now = clock_ms();
            sqlx::query("INSERT INTO api_cloud_billing_periods VALUES('resume-period','user-bob','polar',NULL,'manual','USD',$1,$2,1000,1000,500,500,$1)").bind(now-1000).bind(now+86400000).execute(&pool).await.unwrap();
            sqlx::query("UPDATE api_cloud_workspaces SET state='paused',desired_state='paused',runtime_state='offline',request_config=jsonb_set(request_config,'{rustLifecycleOperation}',$2) WHERE workspace_id=$1")
                .bind(&workspace).bind(json!({"id":operation,"action":"pause","phase":"paused"})).execute(&pool).await.unwrap();
            let access = crate::access::Access {
                actor: crate::auth::Principal {
                    account_id: "user-bob".into(),
                },
                scope: crate::access::Scope::Personal,
                owner_id: "user-bob".into(),
                membership: None,
            };
            let command = "actual-idempotent-resume";
            let returned = control
                .lifecycle_action(&workspace, &access, Action::Resume, Some(command), now)
                .await
                .unwrap();
            assert_eq!(returned["state"], "provisioning");
            assert_eq!(returned["desiredState"], "ready");
            control
                .lifecycle_action(&workspace, &access, Action::Resume, Some(command), now + 1)
                .await
                .unwrap();
            let (count,jobs,max_attempts):(i64,i64,i32)=sqlx::query_as("SELECT count(DISTINCT o.operation_id),count(j.job_id),min(j.max_attempts) FROM zuse_rust_lifecycle_operations o JOIN zuse_rust_jobs j ON j.payload->>'operationId'=o.operation_id WHERE o.workspace_id=$1 AND o.command_id=$2 AND j.state='pending'").bind(&workspace).bind(command).fetch_one(&pool).await.unwrap();
            assert_eq!((count, jobs, max_attempts), (1, 1, 120));
            assert_eq!(
                control
                    .lifecycle_action(&workspace, &access, Action::Pause, Some(command), now + 2)
                    .await
                    .unwrap_err()
                    .code,
                "cloud_lifecycle_command_id_reused"
            );
        }
        pool.close().await;
        sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
            .execute(&admin)
            .await
            .unwrap();
    }
    admin.close().await;
}
