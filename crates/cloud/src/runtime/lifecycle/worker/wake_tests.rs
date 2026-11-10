use super::tests::{Remote, control};
use super::*;
use std::sync::{
    Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};

struct WakeRemote {
    base: Remote,
    launches: AtomicUsize,
    case: &'static str,
}
#[async_trait::async_trait]
impl RetainedProvider for WakeRemote {
    fn provider_id(&self) -> &'static str {
        "boxd"
    }
    async fn inspect_retained(&self, id: &str) -> Result<Option<Machine>, ProviderError> {
        self.base.inspect_retained(id).await
    }
    async fn pause_retained(&self, _: &str) -> Result<(), ProviderError> {
        panic!("wake only")
    }
    async fn wake_retained(&self, _: &str, _: &MachineState) -> Result<(), ProviderError> {
        panic!("receipt required")
    }
    async fn launch_retained(
        &self,
        d: &Directive,
        workspace: &str,
        _: &str,
    ) -> Result<(), ProviderError> {
        self.base.assert_unlocked().await;
        assert_eq!(workspace, self.base.workspace);
        assert_eq!(d.source_machine_id, "memory-source");
        self.launches.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    async fn wake_receipt(
        &self,
        id: &str,
        _: &MachineState,
    ) -> Result<ProviderOperation, ProviderError> {
        self.base.assert_unlocked().await;
        assert_eq!(id, "memory-source");
        self.base.calls.fetch_add(1, Ordering::SeqCst);
        *self.base.machine.lock().unwrap() = MachineState::Running;
        if self.case == "uncertain" {
            return Err(ProviderError::OutcomeUnknown);
        }
        Ok(ProviderOperation {
            resource_id: id.into(),
            operation_id: Some("original-wake".into()),
            desired: DesiredState::Running,
        })
    }
    async fn receipt_state(&self, r: &ProviderOperation) -> Result<OperationState, ProviderError> {
        self.base.assert_unlocked().await;
        assert_eq!(r.resource_id, "memory-source");
        assert_eq!(r.desired, DesiredState::Running);
        self.base
            .observations
            .lock()
            .unwrap()
            .push(r.operation_id.clone());
        if self.case == "uncertain" {
            assert_eq!(r.operation_id, None);
            return Ok(OperationState::Retrying);
        }
        assert_eq!(r.operation_id.as_deref(), Some("original-wake"));
        // The accepted original receipt must exist before any observation.
        let saved:Value=sqlx::query_scalar("SELECT provider_effects->'wake' FROM zuse_rust_lifecycle_operations WHERE workspace_id=$1 AND action='resume'").bind(&self.base.workspace).fetch_one(&self.base.pool).await.unwrap();
        assert_eq!(saved["receipt"]["operationId"], "original-wake");
        assert_eq!(saved["state"], "accepted");
        match self.case {
            "lease-loss" => {
                sqlx::query("UPDATE zuse_rust_jobs SET lease_expires_at=0 WHERE resource_id=$1")
                    .bind(&self.base.workspace)
                    .execute(&self.base.pool)
                    .await
                    .unwrap();
            }
            "delete" => {
                sqlx::query("UPDATE api_cloud_workspaces SET state='deleting',desired_state='deleted' WHERE workspace_id=$1").bind(&self.base.workspace).execute(&self.base.pool).await.unwrap();
            }
            "generation" => {
                sqlx::query("UPDATE api_cloud_workspaces SET request_config=jsonb_set(request_config,'{runtimeGeneration}','3') WHERE workspace_id=$1").bind(&self.base.workspace).execute(&self.base.pool).await.unwrap();
            }
            _ => {}
        }
        Ok(if self.case == "pending" {
            OperationState::Retrying
        } else {
            OperationState::Completed
        })
    }
}

#[tokio::test]
#[ignore = "requires explicit isolated localhost PostgreSQL"]
async fn accepted_personal_ram_wake_launches_in_one_lease_only_after_original_completion() {
    let url = std::env::var("ZUSE_CLOUD_TEST_DATABASE_URL").unwrap();
    assert!(matches!(
        reqwest::Url::parse(&url).unwrap().host_str(),
        Some("localhost" | "127.0.0.1" | "::1")
    ));
    let admin = PgPool::connect(&url).await.unwrap();
    for case in [
        "completed",
        "pending",
        "lease-loss",
        "delete",
        "generation",
        "uncertain",
    ] {
        let schema = format!("wake_{}", uuid::Uuid::new_v4().simple());
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
        let now = clock_ms();
        let workspace = format!("wake-{case}");
        let config = json!({"runtimeGeneration":2,"gatewayEpoch":2,"runtimeRpcProtocol":zuse_protocol::cloud_gateway::RUST_RPC,"runtimeBootstrapReceipt":{"workspaceId":workspace,"generation":2,"gatewayEpoch":2},"runtimeCredentialExpiresAtMs":9007199254740991_i64,"runtimeCredentialKeyThumbprint":"credential-key","runtimeSigningKeyThumbprint":"signing-key","rustRuntimeStorage":{"storageIncarnationId":"memory-disk"},"rustMemorySleepVersion":1,"rustMemoryParked":{"version":1,"operationId":"original-pause","sourceMachineId":"memory-source","generation":2,"storageIncarnationId":"memory-disk"},"rustStorageEnrollment":{"phase":"committed","generation":2,"sourceSandboxId":"memory-source","storageIncarnationId":"memory-disk","credentialKeyThumbprint":"credential-key","signingKeyThumbprint":"signing-key","capabilities":[zuse_protocol::cloud_lifecycle::CAPABILITY,zuse_protocol::cloud_lifecycle::MEMORY_CAPABILITY]},"rustLifecycleOperation":{"id":"original-pause","action":"pause","phase":"paused"}});
        sqlx::query("UPDATE api_cloud_workspaces SET workspace_id=$1,provider='boxd',provider_sandbox_id='memory-source',state='paused',desired_state='paused',runtime_state='offline',request_config=$2 WHERE workspace_id='ws-bob'").bind(&workspace).bind(config).execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO zuse_rust_control_owners VALUES('workspace',$1,'rust',2,0)")
            .bind(&workspace)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO zuse_rust_lifecycle_operations(operation_id,workspace_id,command_id,generation,source_machine_id,storage_incarnation_id,action,phase,created_at,updated_at) VALUES('original-pause',$1,'pause-command',2,'memory-source','memory-disk','pause','paused',$2,$2)").bind(&workspace).bind(now).execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO api_cloud_billing_periods VALUES('wake-period','user-bob','polar',NULL,'manual','USD',$1,$2,1000,1000,500,500,$1)").bind(now-1000).bind(now+86400000).execute(&pool).await.unwrap();
        let control = control(pool.clone());
        let access = crate::access::Access {
            actor: crate::auth::Principal {
                account_id: "user-bob".into(),
            },
            scope: crate::access::Scope::Personal,
            owner_id: "user-bob".into(),
            membership: None,
        };
        control
            .lifecycle_action(
                &workspace,
                &access,
                Action::Resume,
                Some("resume-command"),
                now,
            )
            .await
            .unwrap();
        if case == "completed" {
            sqlx::query(
                "UPDATE api_cloud_workspaces SET runtime_credential_hash=$2 WHERE workspace_id=$1",
            )
            .bind(&workspace)
            .bind(hash("wake-credential"))
            .execute(&pool)
            .await
            .unwrap();
            let scope = schema.clone();
            let one_pool = sqlx::postgres::PgPoolOptions::new()
                .max_connections(1)
                .acquire_timeout(Duration::from_secs(2))
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
            let authority = std::sync::Arc::new(super::tests::control(one_pool.clone()));
            let (_, mut borrowed) = authority
                .runtime_authority_connection(&workspace, &RuntimeRequest::Query)
                .await
                .unwrap();
            assert!(
                borrowed.is_some(),
                "actual RAM Query selects a reusable checkout"
            );
            let backend: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
                .fetch_one(&mut **borrowed.as_mut().unwrap())
                .await
                .unwrap();
            drop(borrowed);
            let value = tokio::time::timeout(
                Duration::from_secs(3),
                authority.lifecycle_runtime_pilot(
                    &workspace,
                    "wake-credential",
                    RuntimeRequest::Query,
                    clock_ms(),
                    None,
                    None,
                ),
            )
            .await
            .unwrap()
            .unwrap();
            assert_eq!(value["directive"]["action"], "resume");
            assert_eq!(
                authority
                    .lifecycle_runtime_pilot(
                        &workspace,
                        "wrong-credential",
                        RuntimeRequest::Query,
                        clock_ms(),
                        None,
                        None
                    )
                    .await
                    .unwrap_err()
                    .code,
                "workspace_runtime_fenced"
            );
            // Preserve generation and fresh-clock fences across a real wait
            // on the workspace row, after the early reusable checkout read.
            for expiry_case in [false, true] {
                let mut locked = pool.begin().await.unwrap();
                let original: Value = sqlx::query_scalar("SELECT request_config FROM api_cloud_workspaces WHERE workspace_id=$1 FOR UPDATE")
                    .bind(&workspace).fetch_one(&mut *locked).await.unwrap();
                let request_at = clock_ms();
                let current = authority.clone();
                let task = workspace.clone();
                let query = tokio::spawn(async move {
                    current
                        .lifecycle_runtime_pilot(
                            &task,
                            "wake-credential",
                            RuntimeRequest::Query,
                            request_at,
                            None,
                            None,
                        )
                        .await
                });
                let mut waiting = false;
                for _ in 0..200 {
                    waiting = sqlx::query_scalar::<_, bool>(
                        "SELECT COALESCE(wait_event_type='Lock',false) FROM pg_stat_activity WHERE pid=$1",
                    )
                    .bind(backend)
                    .fetch_optional(&pool)
                    .await
                    .unwrap()
                    .unwrap_or(false);
                    if waiting {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
                assert!(
                    waiting,
                    "actual authority CTE must be blocked on the held row"
                );
                let mut changed = original.clone();
                if expiry_case {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                    changed["runtimeCredentialExpiresAtMs"] = json!(clock_ms() - 1);
                } else {
                    changed["runtimeGeneration"] = json!(3);
                }
                sqlx::query(
                    "UPDATE api_cloud_workspaces SET request_config=$2 WHERE workspace_id=$1",
                )
                .bind(&workspace)
                .bind(changed)
                .execute(&mut *locked)
                .await
                .unwrap();
                locked.commit().await.unwrap();
                assert_eq!(
                    tokio::time::timeout(Duration::from_secs(3), query)
                        .await
                        .unwrap()
                        .unwrap()
                        .unwrap_err()
                        .code,
                    "workspace_runtime_fenced"
                );
                sqlx::query(
                    "UPDATE api_cloud_workspaces SET request_config=$2 WHERE workspace_id=$1",
                )
                .bind(&workspace)
                .bind(original)
                .execute(&pool)
                .await
                .unwrap();
            }
            drop(authority);
            one_pool.close().await;
        }
        let remote = WakeRemote {
            base: Remote {
                pool: pool.clone(),
                workspace: workspace.clone(),
                machine: Mutex::new(MachineState::Standby),
                calls: AtomicUsize::new(0),
                observations: Mutex::new(vec![]),
                complete: AtomicBool::new(true),
                deleted: AtomicBool::new(false),
                expire: false,
                uncertain: false,
            },
            launches: AtomicUsize::new(0),
            case,
        };
        let result = control.lifecycle_once(&remote, clock_ms()).await;
        assert_eq!(
            result.is_ok(),
            matches!(case, "completed" | "pending"),
            "{case}"
        );
        assert_eq!(remote.base.calls.load(Ordering::SeqCst), 1, "{case}");
        assert_eq!(
            remote.launches.load(Ordering::SeqCst),
            usize::from(case == "completed"),
            "{case}"
        );
        let (phase,effect):(String,Value)=sqlx::query_as("SELECT phase,provider_effects->'wake' FROM zuse_rust_lifecycle_operations WHERE action='resume'").fetch_one(&pool).await.unwrap();
        assert_eq!(phase == "launching", case == "completed", "{case}");
        assert_eq!(
            effect["state"],
            if case == "uncertain" {
                "uncertain"
            } else {
                "accepted"
            }
        );
        if case != "uncertain" {
            assert_eq!(effect["receipt"]["operationId"], "original-wake");
        }
        let attempts: i32 =
            sqlx::query_scalar("SELECT attempts FROM zuse_rust_jobs WHERE resource_id=$1")
                .bind(&workspace)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(attempts, 1);
        if case == "pending" {
            let due: i64 =
                sqlx::query_scalar("SELECT due_at FROM zuse_rust_jobs WHERE resource_id=$1")
                    .bind(&workspace)
                    .fetch_one(&pool)
                    .await
                    .unwrap();
            assert!(due >= clock_ms() + 1500);
            assert!(!control.lifecycle_once(&remote, clock_ms()).await.unwrap());
        }
        if case == "uncertain" {
            sqlx::query("UPDATE zuse_rust_jobs SET due_at=0 WHERE resource_id=$1")
                .bind(&workspace)
                .execute(&pool)
                .await
                .unwrap();
            control.lifecycle_once(&remote, clock_ms()).await.unwrap();
            assert_eq!(remote.base.calls.load(Ordering::SeqCst), 1);
            assert_eq!(remote.launches.load(Ordering::SeqCst), 0);
        }
        drop(remote);
        drop(control);
        pool.close().await;
        sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
            .execute(&admin)
            .await
            .unwrap();
    }
    admin.close().await;
}
