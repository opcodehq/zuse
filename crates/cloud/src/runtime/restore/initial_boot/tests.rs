use super::*;
use crate::{crypto::MasterKey, providers::Machine, tickets::TicketSigner};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use std::sync::{
    Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};

struct Probe {
    pool: PgPool,
    mode: &'static str,
    boat: bool,
    calls: AtomicUsize,
    starts: Mutex<Option<String>>,
    installed: AtomicBool,
    uploads: AtomicUsize,
    launches: AtomicUsize,
}
impl Probe {
    async fn check_proof(&self) {
        let c: Value = sqlx::query_scalar(
            "SELECT request_config FROM api_cloud_workspaces WHERE workspace_id='ws-bob'",
        )
        .fetch_one(&self.pool)
        .await
        .unwrap();
        if self.mode != "legacy" {
            assert_eq!(
                c["initialRuntimeBootstrapReceipt"]["sourceArtifactId"],
                "managed-artifact"
            );
            assert_eq!(
                c["initialRuntimeBootstrapReceipt"]["proof"]["ipcVerified"], true,
                "exact helper proof must commit before any archive credential or boot upload"
            );
        }
    }
}
#[async_trait]
impl AllocationProvider for Probe {
    fn provider_id(&self) -> &'static str {
        if self.boat { "boat" } else { "boxd" }
    }
    async fn find(&self, _: &str) -> Result<Option<Machine>, ProviderError> {
        Ok(Some(Machine {
            id: "restore-target".into(),
            name: "restore-original".into(),
            state: MachineState::Running,
            vcpu: None,
            memory_bytes: None,
        }))
    }
    async fn create(&self, _: &str, _: &str) -> Result<Machine, ProviderError> {
        panic!("existing restore target must never be replaced")
    }
    async fn bootstrap_initial_runtime(
        &self,
        machine: &str,
        source: &str,
        name: &str,
        r: &Request,
    ) -> Result<Option<Value>, ProviderError> {
        assert_eq!(
            (machine, source, name),
            ("restore-target", "managed-artifact", "restore-original")
        );
        assert!(r.purpose == Purpose::InitialRestore);
        assert_eq!(r.operation_id, "restore-original");
        assert_eq!(r.storage_incarnation_id, "original-disk");
        assert_eq!(r.generation, 3);
        let digest = r.digest().unwrap();
        {
            let mut original = self.starts.lock().unwrap();
            if let Some(prior) = original.as_ref() {
                assert_eq!(prior, &digest, "reconcile original helper request");
            } else {
                *original = Some(digest.clone());
            }
        }
        // A second connection mutates the row while the helper is running: no SQL lock across I/O.
        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            sqlx::query(
                "UPDATE api_cloud_workspaces SET revision=revision+1 WHERE workspace_id='ws-bob'",
            )
            .execute(&self.pool),
        )
        .await
        .expect("helper must release workspace lock")
        .unwrap();
        let sql = match self.mode {
            "delete" => Some(
                "UPDATE api_cloud_workspaces SET desired_state='deleted',state='deleting' WHERE workspace_id='ws-bob'",
            ),
            "lease-expired" => Some("UPDATE zuse_rust_jobs SET lease_expires_at=0"),
            "lease-replaced" => Some("UPDATE zuse_rust_jobs SET lease_token='different-lease'"),
            "generation" => {
                Some("UPDATE zuse_rust_control_owners SET generation=4 WHERE resource_id='ws-bob'")
            }
            "source" => Some("UPDATE zuse_rust_restores SET snapshot_id='different-artifact'"),
            "machine" => Some(
                "UPDATE api_cloud_workspaces SET provider_sandbox_id='different-machine' WHERE workspace_id='ws-bob'",
            ),
            "disk" => Some(
                "UPDATE api_cloud_workspaces SET request_config=jsonb_set(request_config,'{rustRuntimeStorage,storageIncarnationId}','\"different-disk\"') WHERE workspace_id='ws-bob'",
            ),
            "pins" => Some(
                "UPDATE api_cloud_workspaces SET request_config=jsonb_set(request_config,'{initialRuntimeBootstrap,helperSha256}',to_jsonb(repeat('f',64))) WHERE workspace_id='ws-bob'",
            ),
            "enrolled" => Some(
                "UPDATE api_cloud_workspaces SET runtime_credential_hash='new-binding' WHERE workspace_id='ws-bob'",
            ),
            _ => None,
        };
        if let Some(sql) = sql {
            sqlx::query(sql).execute(&self.pool).await.unwrap();
        }
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        if self.mode == "reply-lost" && call == 0 {
            return Err(ProviderError::OutcomeUnknown);
        }
        if self.mode == "pending" && call == 0 {
            return Ok(None);
        }
        let mut response = serde_json::to_value(r).unwrap();
        response["status"] = json!("ready");
        response["requestSha256"] = json!(digest);
        response["unitStartSubmitted"] = json!(true);
        response["resident"] = json!({"residentPid":42,"residentStartTicks":1,"bootNonceSha256":"a".repeat(64),"ipcVerified":true,"unbound":true,"childAlive":false,"recoveryAlive":false});
        r.response(&response)
    }
    async fn launch(&self, _: &str, launch: &RuntimeLaunch) -> Result<(), ProviderError> {
        self.check_proof().await;
        assert_eq!(launch.operation_id, "restore-original");
        assert!(!launch.boot_token.is_empty());
        self.launches.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}
#[async_trait]
impl RecoveryProvider for Probe {
    fn provider_id(&self) -> &'static str {
        if self.boat { "boat" } else { "boxd" }
    }
    async fn inspect(&self, id: &str) -> Result<Option<Machine>, ProviderError> {
        AllocationProvider::find(self, id).await
    }
    async fn wake(&self, _: &str, _: &MachineState) -> Result<(), ProviderError> {
        panic!("running target")
    }
    async fn capture(&self, _: &CapturePlan) -> Result<(), ProviderError> {
        panic!("restore only")
    }
    async fn stop_capture(&self, _: &str, _: &str) -> Result<(), ProviderError> {
        Ok(())
    }
    async fn pause(&self, _: &str) -> Result<(), ProviderError> {
        panic!("no pause")
    }
    async fn retire(&self, _: &str) -> Result<(), ProviderError> {
        panic!("never retire source")
    }
}
#[async_trait]
impl RestoreProvider for Probe {
    async fn installed(&self, _: &str, _: &str) -> Result<bool, ProviderError> {
        Ok(self.installed.load(Ordering::SeqCst))
    }
    async fn restore(&self, machine: &str, plan: &CapturePlan) -> Result<(), ProviderError> {
        self.check_proof().await;
        assert_eq!(machine, "restore-target");
        assert_eq!(plan.operation_id, "archive-original");
        let private: Value = serde_json::from_str(&plan.config).unwrap();
        assert_eq!(private["mode"], "restore");
        assert!(private["token"].is_string() && private["key"].is_string());
        self.uploads.fetch_add(1, Ordering::SeqCst);
        self.installed.store(true, Ordering::SeqCst);
        Ok(())
    }
}

#[tokio::test]
#[ignore = "requires isolated localhost PostgreSQL"]
async fn fresh_restore_boot_reconciles_original_and_fences_credentials_after_helper_io() {
    let url = std::env::var("ZUSE_CLOUD_TEST_DATABASE_URL").unwrap();
    assert!(matches!(
        reqwest::Url::parse(&url).unwrap().host_str(),
        Some("localhost" | "127.0.0.1" | "::1")
    ));
    let admin = PgPool::connect(&url).await.unwrap();
    for variant in [
        "boat-ready",
        "boat-reply-lost",
        "boat-pins",
        "ready",
        "legacy",
        "reply-lost",
        "pending",
        "delete",
        "lease-expired",
        "lease-replaced",
        "generation",
        "source",
        "machine",
        "disk",
        "pins",
        "enrolled",
    ] {
        let boat = variant.starts_with("boat-");
        let mode = variant.strip_prefix("boat-").unwrap_or(variant);
        let schema = format!("restore_boot_{}", uuid::Uuid::new_v4().simple());
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
            crate::runtime::lifecycle::MIGRATION,
            crate::runtime::recovery::MIGRATION,
            crate::runtime::restore::MIGRATION,
            include_str!("../../../../migrations/0012_provider_receipts.sql"),
            include_str!("../../../../migrations/0013_recovery_receipts.sql"),
            include_str!("../../../../migrations/0014_restore_receipts.sql"),
        ] {
            sqlx::raw_sql(sql).execute(&pool).await.unwrap();
        }
        sqlx::query("INSERT INTO api_cloud_billing_periods(period_id,account_id,status,period_start,period_end,base_price_micros,included_provider_cost_micros,markup_basis_points,overage_cap_micros,updated_at) VALUES('period','user-bob','active',0,9999999999999,0,1000000000,0,0,0)").execute(&pool).await.unwrap();
        let signing = ed25519_dalek::SigningKey::from_bytes(&[74; 32]);
        let jwk = json!({"kty":"OKP","crv":"Ed25519","d":URL_SAFE_NO_PAD.encode([74;32]),"x":URL_SAFE_NO_PAD.encode(signing.verifying_key().as_bytes())});
        let control = RuntimeControl::from_pool(
            pool.clone(),
            MasterKey::parse(&URL_SAFE_NO_PAD.encode([75; 32])).unwrap(),
            TicketSigner::from_jwk(&jwk.to_string(), "https://api.example.test").unwrap(),
        );
        let (_, wrapped) = control
            .master
            .create_transcript_key("user-bob", "ws-bob")
            .unwrap();
        let manifest = json!({"version":1,"operationId":"archive-original","workspaceId":"ws-bob","chatId":"chat-bob","sessionId":"session-bob","chunks":["a".repeat(64)],"archiveSha256":"b".repeat(64)});
        let archive = json!({"version":1,"id":"archive-original","sourceSandboxId":"old-source","archivedAtMs":1,"expiresAtMs":9999999999999_i64,"phase":"restoring","verifiedChunks":1,"manifest":manifest});
        let binding = json!({"operationId":"archive-original","workspaceId":"ws-bob","chatId":"chat-bob","sessionId":"session-bob","rustStorage":{"accountId":"user-bob","database":"engine.sqlite","storageIncarnationId":"original-disk"}});
        let mut config = json!({"runtimeGeneration":3,"gatewayEpoch":3,"rustProviderControllerVersion":1,"managedReleaseId":"explicit-fresh-release","recoveryArchive":archive,"rustRuntimeStorage":{"storageIncarnationId":"original-disk"},"rustRestoreOperationId":"restore-original","rustLifecycleOperation":{"id":"restore-original","action":"resume","phase":"waking"}});
        if mode != "legacy" {
            config["initialRuntimeBootstrap"] = json!({"version":1,"mode":if boat {"post-hydration-first-start-v1"} else {"stopped-memory-snapshot-v1"},"runtimeSha256":"a".repeat(64),"supervisorSha256":"b".repeat(64),"serviceSha256":"c".repeat(64),"manifestSha256":"d".repeat(64),"helperSha256":"e".repeat(64)});
        }
        sqlx::query("UPDATE api_cloud_workspaces SET provider=$3,state='provisioning',desired_state='ready',runtime_state='offline',provider_sandbox_id='restore-target',runtime_credential_hash=NULL,runtime_boot_token_hash=NULL,request_config=$1,wrapped_transcript_key=$2 WHERE workspace_id='ws-bob'").bind(config).bind(wrapped).bind(if boat {"boat"} else {"boxd"}).execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO zuse_rust_control_owners VALUES('workspace','ws-bob','rust',3,0)")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO zuse_rust_recovery_operations(operation_id,workspace_id,generation,restore_generation,source_machine_id,storage_incarnation_id,binding,phase,manifest,expires_at,verified_at,verified_archive_sha256,created_at,updated_at) VALUES('archive-original','ws-bob',2,3,'old-source','original-disk',$1,'restoring',$2,9999999999999,1,$3,1,1)").bind(binding).bind(manifest).bind("b".repeat(64)).execute(&pool).await.unwrap();
        let boot = control
            .master
            .seal_api(
                &context("ws-bob", "restore-original", 3),
                "private-test-boot",
            )
            .unwrap();
        sqlx::query("INSERT INTO zuse_rust_restores(operation_id,archive_operation_id,workspace_id,generation,command_id,snapshot_id,machine_name,machine_id,sealed_boot_token,boot_expires_at,phase,created_at,updated_at) VALUES('restore-original','archive-original','ws-bob',3,'restore-command','managed-artifact','restore-original','restore-target',$1,9999999999999,'allocated',1,1)").bind(boot).execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO zuse_rust_lifecycle_operations(operation_id,workspace_id,command_id,generation,source_machine_id,storage_incarnation_id,action,phase,created_at,updated_at) VALUES('restore-original','ws-bob','restore-command',3,'restore-target','original-disk','resume','waking',1,1)").execute(&pool).await.unwrap();
        let mut tx = pool.begin().await.unwrap();
        jobs::enqueue(
            &mut tx,
            "restore-test",
            JOB_KIND,
            "ws-bob",
            3,
            &json!({"operationId":"restore-original"}),
            chrono::Utc::now().timestamp_millis(),
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
        let probe = Probe {
            pool: pool.clone(),
            mode,
            boat,
            calls: AtomicUsize::new(0),
            starts: Mutex::new(None),
            installed: AtomicBool::new(false),
            uploads: AtomicUsize::new(0),
            launches: AtomicUsize::new(0),
        };
        let result = control
            .restore_once(&probe, chrono::Utc::now().timestamp_millis())
            .await;
        let success = matches!(mode, "ready" | "legacy");
        if success {
            assert!(result.is_ok(), "{mode}: {result:?}");
            assert_eq!(probe.uploads.load(Ordering::SeqCst), 1);
        } else {
            assert_eq!(probe.uploads.load(Ordering::SeqCst), 0, "{mode}");
            if mode != "pending" {
                assert!(result.is_err(), "{mode}");
            }
        }
        if success || matches!(mode, "pending" | "reply-lost") {
            let retries = if success { 2 } else { 3 };
            for _ in 0..retries {
                sqlx::query("UPDATE zuse_rust_jobs SET due_at=0")
                    .execute(&pool)
                    .await
                    .unwrap();
                control
                    .restore_once(&probe, chrono::Utc::now().timestamp_millis())
                    .await
                    .unwrap();
            }
            assert_eq!(
                probe.uploads.load(Ordering::SeqCst),
                1,
                "archive credential upload only once"
            );
            assert_eq!(probe.launches.load(Ordering::SeqCst), 1);
            assert_eq!(
                probe.calls.load(Ordering::SeqCst),
                if mode == "legacy" {
                    0
                } else if success {
                    1
                } else {
                    2
                }
            );
        } else {
            assert_eq!(probe.launches.load(Ordering::SeqCst), 0);
            let c: Value = sqlx::query_scalar(
                "SELECT request_config FROM api_cloud_workspaces WHERE workspace_id='ws-bob'",
            )
            .fetch_one(&pool)
            .await
            .unwrap();
            assert!(c["initialRuntimeBootstrapReceipt"].is_null());
            assert!(c["rustRecoveryRestore"].is_null());
        }
        pool.close().await;
        sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
            .execute(&admin)
            .await
            .unwrap();
    }
    admin.close().await;
}
