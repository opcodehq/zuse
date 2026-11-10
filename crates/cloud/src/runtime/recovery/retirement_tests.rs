use super::*;
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

struct Remote {
    pool: PgPool,
    deleted: AtomicBool,
    complete: AtomicBool,
    calls: AtomicUsize,
    expire: bool,
    uncertain: bool,
}
impl Remote {
    async fn unlocked(&self) {
        tokio::time::timeout(
            Duration::from_secs(2),
            sqlx::query(
                "UPDATE api_cloud_workspaces SET revision=revision+1 WHERE workspace_id='ws-bob'",
            )
            .execute(&self.pool),
        )
        .await
        .expect("retirement provider call held a database row lock")
        .unwrap();
    }
}
#[async_trait]
impl RecoveryProvider for Remote {
    async fn inspect(&self, id: &str) -> Result<Option<Machine>, ProviderError> {
        self.unlocked().await;
        Ok((!self.deleted.load(Ordering::SeqCst)).then(|| Machine {
            id: id.into(),
            name: "original".into(),
            state: MachineState::Paused,
            vcpu: None,
            memory_bytes: None,
        }))
    }
    async fn wake(&self, _: &str, _: &MachineState) -> Result<(), ProviderError> {
        panic!("retirement must not wake source")
    }
    async fn capture(&self, _: &CapturePlan) -> Result<(), ProviderError> {
        panic!("already verified")
    }
    async fn stop_capture(&self, _: &str, _: &str) -> Result<(), ProviderError> {
        panic!("no unfenced error cleanup")
    }
    async fn pause(&self, _: &str) -> Result<(), ProviderError> {
        panic!("already paused")
    }
    async fn retire(&self, _: &str) -> Result<(), ProviderError> {
        panic!("receipt required")
    }
    async fn retire_receipt(&self, id: &str) -> Result<ProviderOperation, ProviderError> {
        self.unlocked().await;
        assert_eq!(id, "machine-bob");
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.deleted.store(true, Ordering::SeqCst);
        if self.expire {
            sqlx::query("UPDATE zuse_rust_jobs SET lease_expires_at=0 WHERE resource_id='ws-bob'")
                .execute(&self.pool)
                .await
                .unwrap();
        }
        if self.uncertain {
            return Err(ProviderError::OutcomeUnknown);
        }
        Ok(ProviderOperation {
            resource_id: id.into(),
            operation_id: Some("original-retirement".into()),
            desired: DesiredState::Deleted,
        })
    }
    async fn retirement_state(
        &self,
        r: &ProviderOperation,
    ) -> Result<OperationState, ProviderError> {
        self.unlocked().await;
        assert_eq!(r.resource_id, "machine-bob");
        assert_eq!(r.desired, DesiredState::Deleted);
        assert_eq!(
            r.operation_id.as_deref(),
            if self.uncertain {
                None
            } else {
                Some("original-retirement")
            }
        );
        Ok(if self.complete.load(Ordering::SeqCst) {
            OperationState::Completed
        } else {
            OperationState::Pending
        })
    }
}
struct PendingPause<'a> {
    original: &'a Remote,
    sleeping: AtomicBool,
    calls: AtomicUsize,
    complete: AtomicBool,
}
#[async_trait]
impl RecoveryProvider for PendingPause<'_> {
    async fn inspect(&self, id: &str) -> Result<Option<Machine>, ProviderError> {
        self.original.unlocked().await;
        Ok(Some(Machine {
            id: id.into(),
            name: "original".into(),
            state: if self.sleeping.load(Ordering::SeqCst) {
                MachineState::Stopped
            } else {
                MachineState::Running
            },
            vcpu: None,
            memory_bytes: None,
        }))
    }
    async fn wake(&self, _: &str, _: &MachineState) -> Result<(), ProviderError> {
        panic!("verification must not wake")
    }
    async fn capture(&self, _: &CapturePlan) -> Result<(), ProviderError> {
        panic!("already uploaded")
    }
    async fn stop_capture(&self, _: &str, _: &str) -> Result<(), ProviderError> {
        self.original.unlocked().await;
        Ok(())
    }
    async fn pause(&self, _: &str) -> Result<(), ProviderError> {
        panic!("typed Stop receipt required")
    }
    async fn retire(&self, _: &str) -> Result<(), ProviderError> {
        panic!("Stop is not retirement")
    }
    async fn pause_receipt(&self, id: &str) -> Result<ProviderOperation, ProviderError> {
        self.original.unlocked().await;
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.sleeping.store(true, Ordering::SeqCst);
        if self.original.expire {
            sqlx::query("UPDATE zuse_rust_jobs SET lease_expires_at=0 WHERE resource_id='ws-bob'")
                .execute(&self.original.pool)
                .await
                .unwrap();
        }
        if self.original.uncertain {
            return Err(ProviderError::OutcomeUnknown);
        }
        Ok(ProviderOperation {
            resource_id: id.into(),
            operation_id: Some("original-archive-stop".into()),
            desired: DesiredState::Sleeping,
        })
    }
    async fn recovery_state(&self, r: &ProviderOperation) -> Result<OperationState, ProviderError> {
        self.original.unlocked().await;
        assert_eq!(r.resource_id, "machine-bob");
        assert_eq!(r.desired, DesiredState::Sleeping);
        if self.original.uncertain {
            assert!(r.operation_id.is_none());
            return Err(ProviderError::InvalidResponse);
        }
        assert_eq!(r.operation_id.as_deref(), Some("original-archive-stop"));
        Ok(if self.complete.load(Ordering::SeqCst) {
            OperationState::Completed
        } else {
            OperationState::Retrying
        })
    }
}

fn control(pool: PgPool) -> RuntimeControl {
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
async fn retirement_preserves_original_receipt_and_verification_barrier_after_lease_loss() {
    let url = std::env::var("ZUSE_CLOUD_TEST_DATABASE_URL").unwrap();
    assert!(matches!(
        reqwest::Url::parse(&url).unwrap().host_str(),
        Some("localhost" | "127.0.0.1" | "::1")
    ));
    let admin = PgPool::connect(&url).await.unwrap();
    for (expire, uncertain) in [(false, false), (true, false), (true, true)] {
        let schema = format!("retirement_{}", uuid::Uuid::new_v4().simple());
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
        for migration in [
            include_str!("../../../tests/fixtures/legacy-catalog.sql"),
            jobs::MIGRATION,
            lifecycle::MIGRATION,
            MIGRATION,
            include_str!("../../../migrations/0013_recovery_receipts.sql"),
        ] {
            sqlx::raw_sql(migration).execute(&pool).await.unwrap();
        }
        let manifest = json!({"version":1,"operationId":"archive-original","workspaceId":"ws-bob","chatId":"chat-bob","sessionId":"session-bob","chunks":["a".repeat(64)],"archiveSha256":"b".repeat(64)});
        let archive = json!({"version":1,"id":"archive-original","sourceSandboxId":"machine-bob","archivedAtMs":1,"expiresAtMs":9999999999999_i64,"phase":"retiring","verifiedChunks":1,"manifest":manifest});
        let config = json!({"runtimeGeneration":2,"gatewayEpoch":2,"recoveryArchive":archive,"rustRuntimeStorage":{"storageIncarnationId":"original-disk"}});
        sqlx::query("UPDATE api_cloud_workspaces SET state='archived',desired_state='archived',request_config=$1 WHERE workspace_id='ws-bob'").bind(config).execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO zuse_rust_control_owners VALUES('workspace','ws-bob','rust',2,0)")
            .execute(&pool)
            .await
            .unwrap();
        let binding = json!({"workspaceId":"ws-bob","chatId":"chat-bob","sessionId":"session-bob","rustStorage":{"accountId":"user-bob"}});
        sqlx::query("INSERT INTO zuse_rust_recovery_operations(operation_id,workspace_id,generation,source_machine_id,storage_incarnation_id,binding,phase,manifest,expires_at,verified_at,verified_archive_sha256,created_at,updated_at) VALUES('archive-original','ws-bob',2,'machine-bob','original-disk',$1,'retiring',$2,9999999999999,1,$3,1,1)").bind(binding).bind(&manifest).bind("b".repeat(64)).execute(&pool).await.unwrap();
        let mut tx = pool.begin().await.unwrap();
        jobs::enqueue(
            &mut tx,
            "retire-original",
            JOB_KIND,
            "ws-bob",
            2,
            &json!({"operationId":"archive-original"}),
            clock_ms(),
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
        let remote = Remote {
            pool: pool.clone(),
            deleted: AtomicBool::new(false),
            complete: AtomicBool::new(false),
            calls: AtomicUsize::new(0),
            expire,
            uncertain,
        };
        // Provider inspection can say stopped while its original archive Stop
        // is still pending. Never verify/retire or submit another Stop from that hint.
        sqlx::query("UPDATE zuse_rust_recovery_operations SET phase='verifying'")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("UPDATE api_cloud_workspaces SET request_config=jsonb_set(request_config,'{recoveryArchive,phase}','\"verifying\"') WHERE workspace_id='ws-bob'").execute(&pool).await.unwrap();
        let pause = PendingPause {
            original: &remote,
            sleeping: AtomicBool::new(false),
            calls: AtomicUsize::new(0),
            complete: AtomicBool::new(false),
        };
        for attempt in 0..4 {
            sqlx::query("UPDATE zuse_rust_jobs SET due_at=0,lease_expires_at=0")
                .execute(&pool)
                .await
                .unwrap();
            let job = jobs::claim(&pool, JOB_KIND, "workspace", clock_ms(), LEASE)
                .await
                .unwrap()
                .unwrap();
            let result = control(pool.clone())
                .recovery_prepare_verification(&pause, &job)
                .await;
            assert_eq!(result.is_err(), uncertain || expire && attempt == 0);
            if let Ok(ready) = result {
                assert!(!ready, "a stopped appearance cannot settle a pending Stop");
            }
            assert_eq!(pause.calls.load(Ordering::SeqCst), 1);
        }
        if !uncertain {
            pause.complete.store(true, Ordering::SeqCst);
            sqlx::query("UPDATE zuse_rust_jobs SET due_at=0,lease_expires_at=0")
                .execute(&pool)
                .await
                .unwrap();
            let job = jobs::claim(&pool, JOB_KIND, "workspace", clock_ms(), LEASE)
                .await
                .unwrap()
                .unwrap();
            assert!(
                control(pool.clone())
                    .recovery_prepare_verification(&pause, &job)
                    .await
                    .unwrap()
            );
        }
        let pause_effects: Value =
            sqlx::query_scalar("SELECT provider_effects FROM zuse_rust_recovery_operations")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(
            pause_effects["pause"]["state"],
            if uncertain { "uncertain" } else { "completed" }
        );
        assert_eq!(pause.calls.load(Ordering::SeqCst), 1);
        // Reset only the isolated fixture to exercise the retirement stage below.
        sqlx::query(
            "UPDATE zuse_rust_recovery_operations SET phase='retiring',provider_effects='{}'",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("UPDATE api_cloud_workspaces SET request_config=jsonb_set(request_config,'{recoveryArchive,phase}','\"retiring\"') WHERE workspace_id='ws-bob'").execute(&pool).await.unwrap();
        sqlx::query("UPDATE zuse_rust_jobs SET due_at=0,lease_expires_at=0")
            .execute(&pool)
            .await
            .unwrap();
        // A mutated public manifest cannot authorize retirement against the old verification.
        sqlx::query("UPDATE api_cloud_workspaces SET request_config=jsonb_set(request_config,'{recoveryArchive,manifest,archiveSha256}',to_jsonb($1::text)) WHERE workspace_id='ws-bob'").bind("c".repeat(64)).execute(&pool).await.unwrap();
        assert!(
            control(pool.clone())
                .recovery_once(&remote, clock_ms())
                .await
                .is_err()
        );
        assert_eq!(remote.calls.load(Ordering::SeqCst), 0);
        sqlx::query("UPDATE api_cloud_workspaces SET request_config=jsonb_set(request_config,'{recoveryArchive,manifest}', $1) WHERE workspace_id='ws-bob'").bind(&manifest).execute(&pool).await.unwrap();
        sqlx::query("UPDATE zuse_rust_jobs SET due_at=0")
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(
            control(pool.clone())
                .recovery_once(&remote, clock_ms())
                .await
                .is_err(),
            expire || uncertain
        );
        assert_eq!(remote.calls.load(Ordering::SeqCst), 1);
        let effects: Value =
            sqlx::query_scalar("SELECT provider_effects FROM zuse_rust_recovery_operations")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(
            effects["retirement"]["state"],
            if uncertain { "uncertain" } else { "accepted" }
        );
        for complete in [false, false, true] {
            remote.complete.store(complete, Ordering::SeqCst);
            sqlx::query("UPDATE zuse_rust_jobs SET due_at=0")
                .execute(&pool)
                .await
                .unwrap();
            control(pool.clone())
                .recovery_once(&remote, clock_ms())
                .await
                .unwrap();
            let phase: String =
                sqlx::query_scalar("SELECT phase FROM zuse_rust_recovery_operations")
                    .fetch_one(&pool)
                    .await
                    .unwrap();
            assert_eq!(phase, if complete { "retired" } else { "retiring" });
            assert_eq!(
                remote.calls.load(Ordering::SeqCst),
                1,
                "must observe original retirement rather than deleting twice"
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
