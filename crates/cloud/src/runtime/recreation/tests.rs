use super::*;
use crate::{access::Scope, auth::Principal, crypto::MasterKey, tickets::TicketSigner};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
struct NeverObjects;
#[async_trait::async_trait]
impl crate::objects::Objects for NeverObjects {
    async fn get(&self, _: &str) -> Result<Option<Vec<u8>>, ApiError> {
        panic!("test never reads unverified backup")
    }
    async fn put(&self, _: &str, _: &[u8]) -> Result<(), ApiError> {
        panic!("test never writes backup")
    }
}
#[tokio::test]
#[ignore = "requires isolated localhost PostgreSQL"]
async fn computer_recreation_admission_and_verification_fault_matrix() {
    let url = std::env::var("ZUSE_CLOUD_TEST_DATABASE_URL").unwrap();
    assert!(matches!(
        reqwest::Url::parse(&url).unwrap().host_str(),
        Some("localhost" | "127.0.0.1" | "::1")
    ));
    let admin = PgPool::connect(&url).await.unwrap();
    let schema = format!("recreate_{}", uuid::Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&admin)
        .await
        .unwrap();
    let scoped = schema.clone();
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(5)
        .after_connect(move |c, _| {
            let scoped = scoped.clone();
            Box::pin(async move {
                sqlx::query(&format!("SET search_path={scoped}"))
                    .execute(c)
                    .await?;
                Ok(())
            })
        })
        .connect(&url)
        .await
        .unwrap();
    for sql in [
        include_str!("../../../tests/fixtures/legacy-catalog.sql"),
        include_str!("../../../tests/fixtures/billing.sql"),
        jobs::MIGRATION,
        crate::gateway::MIGRATION,
        super::super::mailbox::MIGRATION,
        super::super::lifecycle::MIGRATION,
        super::super::recovery::MIGRATION,
        super::super::restore::MIGRATION,
        include_str!("../../../migrations/0012_provider_receipts.sql"),
        include_str!("../../../migrations/0013_recovery_receipts.sql"),
        include_str!("../../../migrations/0014_restore_receipts.sql"),
        "ALTER TABLE api_cloud_workspaces ADD COLUMN managed_release_id text;CREATE TABLE cloud_managed_bases(provider text,release_id text,artifact_id text,state text,runtime_manifest jsonb);CREATE TABLE zuse_rust_account_event_outbox(owner_id text,kind text,resource_id text,workspace_id text,payload jsonb,created_at bigint)",
        include_str!("../../../migrations/0023_managed_base_admission_lock.sql"),
    ] {
        sqlx::raw_sql(sql).execute(&pool).await.unwrap();
    }
    let now = chrono::Utc::now().timestamp_millis();
    let signing = ed25519_dalek::SigningKey::from_bytes(&[91; 32]);
    let jwk = json!({"kty":"OKP","crv":"Ed25519","d":URL_SAFE_NO_PAD.encode([91;32]),"x":URL_SAFE_NO_PAD.encode(signing.verifying_key().as_bytes())});
    let mut c = RuntimeControl::from_pool(
        pool.clone(),
        MasterKey::parse(&URL_SAFE_NO_PAD.encode([92; 32])).unwrap(),
        TicketSigner::from_jwk(&jwk.to_string(), "https://api.example.test").unwrap(),
    );
    c.recovery_enabled = true;
    c.transcript_objects = Some(Arc::new(NeverObjects));
    // Optional schema absence cannot break ordinary worker boot/capabilities.
    assert!(!c.recreation_once(now).await.unwrap());
    sqlx::raw_sql(MIGRATION).execute(&pool).await.unwrap();
    assert!(
        !c.recreation_once(now).await.unwrap(),
        "absent schema cached until service restart"
    );
    c.recreation_schema_ready.take();
    let release:crate::managed_bases::ManagedBaseRelease=serde_json::from_value(json!({"provider":"boxd","releaseId":"base","artifactId":"snapshot","architecture":"x86_64","certificationReportSha256":"a".repeat(64),"capabilities":{"id":"boxd","configured":true,"managedBaseReady":true,"preservesDisk":true,"preservesMemory":true,"supportsSnapshots":true,"supportsDocker":true,"sizes":[{"id":"default","label":"Fixture","vcpus":2,"memoryMib":2048,"diskGib":20}]},"harnesses":[{"agent":"codex","version":"fixture","sha256":"b".repeat(64),"catalog":{"id":"codex","name":"Codex","defaultModel":"fixture","models":[{"id":"fixture","label":"Fixture"}]}}],"manifest":{"managedBase":true,"rustRuntimeSupervisorVersion":1,"version":1,"buildId":"base","templateVersion":"fixture","configurationDigest":"fixture","planDigest":"c".repeat(64),"runtimeRpcProtocol":"zuse-rpc-v1","rustRuntimeStorageFormat":zuse_protocol::cloud::RUST_RUNTIME_STORAGE_FORMAT,"rustSessionCommandVersion":1,"credentialsRemoved":true,"repositories":[],"rustRepositoryFetchVersion":1,"rustEnvironmentRecipeVersion":1,"rustLaunchModelConfigVersion":1,"rustLaunchAssetsVersion":1}})).unwrap();
    release.validate().unwrap();
    sqlx::query("INSERT INTO cloud_managed_bases VALUES('boxd','base','snapshot','verified',$1)")
        .bind(json!(release))
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO zuse_rust_control_owners VALUES('workspace','ws-bob','rust',1,$1)")
        .bind(now)
        .execute(&pool)
        .await
        .unwrap();
    let (_, wrapped) = c
        .master
        .create_transcript_key("user-bob", "ws-bob")
        .unwrap();
    let config = json!({"runtimeGeneration":1,"gatewayEpoch":1,"runtimeRpcProtocol":zuse_protocol::cloud_gateway::RUST_RPC,"runtimeBootstrapReceipt":{"workspaceId":"ws-bob","generation":1,"gatewayEpoch":1},"runtimeCredentialKeyThumbprint":"key","runtimeSigningKeyThumbprint":"signing","managedReleaseId":"base","providerAuthMode":"broker-v1","rustRecreationVersion":1,"rustMemorySleepVersion":1,"rustRuntimeStorage":{"format":zuse_protocol::cloud::RUST_RUNTIME_STORAGE_FORMAT,"storageIncarnationId":"disk"},"rustStorageEnrollment":{"phase":"committed","generation":1,"sourceSandboxId":"machine-bob","storageIncarnationId":"disk","credentialKeyThumbprint":"key","signingKeyThumbprint":"signing","capabilities":[recovery::CAPABILITY,zuse_protocol::cloud_lifecycle::CAPABILITY,zuse_protocol::cloud_lifecycle::RECREATION_CAPABILITY]}});
    sqlx::query("UPDATE api_cloud_workspaces SET managed_release_id='base',request_config=$1,wrapped_transcript_key=$2 WHERE workspace_id='ws-bob'").bind(&config).bind(wrapped).execute(&pool).await.unwrap();
    let bob = Access {
        actor: Principal {
            account_id: "user-bob".into(),
        },
        scope: Scope::Personal,
        owner_id: "user-bob".into(),
        membership: None,
    };
    let key = uuid::Uuid::new_v4().to_string();
    let op = c
        .recreate_computer("ws-bob", &bob, &key, now)
        .await
        .unwrap();
    let id = op["operationId"].as_str().unwrap();
    assert_eq!(op["state"], "quiescing");
    assert_eq!(
        c.recreate_computer("ws-bob", &bob, &key, now + 1)
            .await
            .unwrap()["operationId"],
        id
    );
    assert!(
        c.recreate_computer("ws-bob", &bob, &uuid::Uuid::new_v4().to_string(), now + 1)
            .await
            .is_err()
    );
    let pause: Value = sqlx::query_scalar(
        "SELECT to_jsonb(l) FROM zuse_rust_lifecycle_operations l WHERE action='pause'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(pause["provider_effects"]["recreationFullShutdown"], true);
    // No in-process Parked receipt can authorize this backup barrier.
    sqlx::query("UPDATE api_cloud_workspaces SET state='paused',runtime_state='offline',request_config=jsonb_set(request_config,'{rustLifecycleOperation,phase}','\"paused\"') WHERE workspace_id='ws-bob'").execute(&pool).await.unwrap();
    sqlx::query("UPDATE zuse_rust_lifecycle_operations SET phase='paused' WHERE operation_id=$1")
        .bind(pause["operation_id"].as_str())
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE zuse_rust_jobs SET state='completed' WHERE kind=$1 AND resource_id='ws-bob'",
    )
    .bind(lifecycle::JOB_KIND)
    .execute(&pool)
    .await
    .unwrap();
    assert!(c.recreation_once(now + 2).await.unwrap());
    let mut row: Value = sqlx::query_scalar(
        "SELECT to_jsonb(w) FROM api_cloud_workspaces w WHERE workspace_id='ws-bob'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(
        capture_intent(&row),
        "stage={}, code={}",
        row["request_config"]["rustRecreation"]["phase"],
        row["status_code"]
    );
    assert!(row["archive_delete_at"].is_null());
    assert_eq!(row["desired_state"], "paused");
    assert_eq!(row["provider_sandbox_id"], "machine-bob");
    let archive = recovery::archive(&row).unwrap();
    assert!(!archive.backup_verified());
    let mut tx = pool.begin().await.unwrap();
    assert!(
        c.recreation_pre_retirement(&mut tx, &row, &archive, now + 3)
            .await
            .is_err()
    );
    tx.rollback().await.unwrap();
    assert!(c.recreation_once(now + 3).await.unwrap());
    assert!(c.recreation_once(now + 2000).await.unwrap());
    assert_eq!(
        sqlx::query_scalar::<_, String>(
            "SELECT phase FROM zuse_rust_computer_recreations WHERE operation_id=$1"
        )
        .bind(id)
        .fetch_one(&pool)
        .await
        .unwrap(),
        "capturing"
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM zuse_rust_restores")
            .fetch_one(&pool)
            .await
            .unwrap(),
        0
    );
    // Broken incarnation/source proof stops work and holds all command leases;
    // no provider effect is even represented until the backup is verified.
    row["request_config"]["rustRecreation"]["storageIncarnationId"] = json!("foreign-disk");
    assert!(!capture_intent(&row));
    sqlx::query("UPDATE api_cloud_workspaces SET request_config=jsonb_set(request_config,'{rustRuntimeStorage,storageIncarnationId}','\"foreign-disk\"') WHERE workspace_id='ws-bob'").execute(&pool).await.unwrap();
    c.recreation_once(now + 4000).await.unwrap();
    let broken: Value = sqlx::query_scalar(
        "SELECT to_jsonb(w) FROM api_cloud_workspaces w WHERE workspace_id='ws-bob'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(leases_blocked(&broken));
    assert_eq!(
        broken["request_config"]["rustRecreation"]["phase"],
        "attention-required"
    );
    assert_eq!(broken["provider_sandbox_id"], "machine-bob");
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM zuse_rust_computer_recreations")
            .fetch_one(&pool)
            .await
            .unwrap(),
        1
    );
    // Repair the fixture's intentionally broken identity, then retry exactly
    // the same immutable operation. No second capture or computer is admitted.
    sqlx::query("UPDATE api_cloud_workspaces SET request_config=jsonb_set(request_config,'{rustRuntimeStorage,storageIncarnationId}','\"disk\"') WHERE workspace_id='ws-bob'").execute(&pool).await.unwrap();
    assert_eq!(
        c.retry_recreation("ws-bob", id, &bob, now + 5000)
            .await
            .unwrap()["operationId"],
        id
    );
    let manifest = json!({"version":1,"operationId":archive.id,"workspaceId":"ws-bob","chatId":"chat-bob","sessionId":"session-bob","chunks":["a".repeat(64)],"archiveSha256":"b".repeat(64)});
    let mut verified = archive.clone();
    verified.phase = ArchivePhase::Retiring;
    verified.verified_chunks = 1;
    verified.manifest = Some(serde_json::from_value(manifest.clone()).unwrap());
    sqlx::query("UPDATE api_cloud_workspaces SET request_config=jsonb_set(request_config,'{recoveryArchive}',$1) WHERE workspace_id='ws-bob'").bind(json!(verified)).execute(&pool).await.unwrap();
    sqlx::query("UPDATE zuse_rust_recovery_operations SET phase='retiring',manifest=$1,verified_at=$2,verified_archive_sha256=$3 WHERE operation_id=$4").bind(&manifest).bind(now+5000).bind("b".repeat(64)).bind(&archive.id).execute(&pool).await.unwrap();
    c.recreation_once(now + 6000).await.unwrap();
    let remote = LostRetirement {
        pool: pool.clone(),
        calls: Default::default(),
        completed: Default::default(),
    };
    // Even an apparently verified public archive cannot substitute another
    // digest for the verifier's exact durable receipt.
    sqlx::query("UPDATE api_cloud_workspaces SET request_config=jsonb_set(request_config,'{recoveryArchive,manifest,archiveSha256}',to_jsonb($1::text)) WHERE workspace_id='ws-bob'").bind("c".repeat(64)).execute(&pool).await.unwrap();
    assert!(c.recovery_once(&remote, now + 6001).await.is_err());
    assert_eq!(remote.calls.load(std::sync::atomic::Ordering::SeqCst), 0);
    sqlx::query("UPDATE api_cloud_workspaces SET request_config=jsonb_set(request_config,'{recoveryArchive,manifest}',$1) WHERE workspace_id='ws-bob'").bind(&manifest).execute(&pool).await.unwrap();
    sqlx::query("UPDATE zuse_rust_jobs SET due_at=0,lease_expires_at=0 WHERE kind=$1")
        .bind(recovery::JOB_KIND)
        .execute(&pool)
        .await
        .unwrap();
    assert!(c.recovery_once(&remote, now + 6002).await.is_err());
    assert_eq!(remote.calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    for complete in [false, true] {
        remote
            .completed
            .store(complete, std::sync::atomic::Ordering::SeqCst);
        sqlx::query("UPDATE zuse_rust_jobs SET due_at=0,lease_expires_at=0 WHERE kind=$1")
            .bind(recovery::JOB_KIND)
            .execute(&pool)
            .await
            .unwrap();
        c.recovery_once(&remote, now + 6003).await.unwrap();
        assert_eq!(
            remote.calls.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "lost retirement reply must inspect original identity, never delete twice"
        );
        let retained: Option<String> = sqlx::query_scalar(
            "SELECT provider_sandbox_id FROM api_cloud_workspaces WHERE workspace_id='ws-bob'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(retained.is_none(), complete);
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM zuse_rust_restores")
                .fetch_one(&pool)
                .await
                .unwrap(),
            0,
            "no replacement before exact provider absence"
        );
    }
    sqlx::query("INSERT INTO api_cloud_billing_periods VALUES('recreate-period','user-bob','polar',NULL,'manual','USD',$1,$2,1000,1000,500,500,$1)").bind(now-1000).bind(now+86400000).execute(&pool).await.unwrap();
    c.recreation_once(now + 8000).await.unwrap();
    let restored: Value = sqlx::query_scalar(
        "SELECT to_jsonb(w) FROM api_cloud_workspaces w WHERE workspace_id='ws-bob'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(restored["request_config"]["runtimeGeneration"], 2);
    assert_eq!(
        restored["request_config"]["rustRuntimeStorage"]["storageIncarnationId"],
        "disk"
    );
    assert_eq!(
        restored["request_config"]["rustRecreation"]["phase"],
        "restoring"
    );
    assert_eq!(restored["branch"], "feature-bob");
    assert_eq!(restored["base_ref"], "main");
    let restore: Value = sqlx::query_scalar("SELECT to_jsonb(r) FROM zuse_rust_restores r")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(restore["snapshot_id"], "snapshot");
    assert_eq!(restore["generation"], 2);
    assert!(restore["machine_id"].is_null());
    c.recreation_once(now + 10000).await.unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM zuse_rust_restores")
            .fetch_one(&pool)
            .await
            .unwrap(),
        1
    );
    sqlx::query("UPDATE zuse_rust_computer_recreations SET phase='ready' WHERE operation_id=$1")
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE api_cloud_workspaces SET request_config=jsonb_set(request_config,'{rustRecreation,phase}','\"ready\"') WHERE workspace_id='ws-bob'").execute(&pool).await.unwrap();
    c.recreation_attention(id, "late-obsolete-error", now + 10001)
        .await
        .unwrap();
    let terminal: Value = sqlx::query_scalar(
        "SELECT to_jsonb(w) FROM api_cloud_workspaces w WHERE workspace_id='ws-bob'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(
        !leases_blocked(&terminal),
        "late failure cannot re-block a completed operation"
    );
    // A sleeping computer first follows its original retained wake path. It
    // never captures a memory-parked Core or allocates another computer.
    let old_pause = uuid::Uuid::new_v4().to_string();
    let mut sleeping = config.clone();
    sleeping["runtimeBootstrapReceipt"]["workspaceId"] = json!("ws-alice");
    sleeping["rustRuntimeStorage"]["storageIncarnationId"] = json!("alice-disk");
    sleeping["rustStorageEnrollment"]["storageIncarnationId"] = json!("alice-disk");
    sleeping["rustStorageEnrollment"]["sourceSandboxId"] = json!("machine-alice");
    sleeping["rustStorageEnrollment"]["capabilities"]
        .as_array_mut()
        .unwrap()
        .push(json!(zuse_protocol::cloud_lifecycle::MEMORY_CAPABILITY));
    sleeping["runtimeCredentialExpiresAtMs"] = json!(now + 3600000);
    sleeping["rustLifecycleOperation"] = json!({"id":old_pause,"action":"pause","phase":"paused"});
    sleeping["rustMemoryParked"] = json!({"version":1,"operationId":old_pause,"generation":1,"sourceMachineId":"machine-alice","storageIncarnationId":"alice-disk"});
    sqlx::query("UPDATE api_cloud_workspaces SET state='paused',desired_state='paused',runtime_state='offline',managed_release_id='base',request_config=$1,runtime_credential_hash=$2,archive_requested_at=NULL,archive_delete_at=NULL WHERE workspace_id='ws-alice'").bind(sleeping).bind(hash("fixture-alice-runtime")).execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO zuse_rust_control_owners VALUES('workspace','ws-alice','rust',1,$1)")
        .bind(now)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO zuse_rust_lifecycle_operations(operation_id,workspace_id,command_id,generation,source_machine_id,storage_incarnation_id,action,phase,created_at,updated_at) VALUES($1,'ws-alice','previous-memory-sleep',1,'machine-alice','alice-disk','pause','paused',$2,$2)").bind(&old_pause).bind(now).execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO api_cloud_billing_periods VALUES('recreate-alice','user-alice','polar',NULL,'manual','USD',$1,$2,1000,1000,500,500,$1)").bind(now-1000).bind(now+86400000).execute(&pool).await.unwrap();
    let alice = Access {
        actor: Principal {
            account_id: "user-alice".into(),
        },
        scope: Scope::Personal,
        owner_id: "user-alice".into(),
        membership: None,
    };
    let asleep = c
        .recreate_computer(
            "ws-alice",
            &alice,
            &uuid::Uuid::new_v4().to_string(),
            now + 11000,
        )
        .await
        .unwrap();
    assert_eq!(asleep["state"], "waking");
    let op: Value = sqlx::query_scalar(
        "SELECT to_jsonb(r) FROM zuse_rust_computer_recreations r WHERE operation_id=$1",
    )
    .bind(asleep["operationId"].as_str())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(op["wake_operation_id"].is_string());
    assert_eq!(op["source_machine_id"], "machine-alice");
    assert_eq!(op["source_generation"], 1);
    c.recreation_once(now + 11001).await.unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM zuse_rust_recovery_operations WHERE workspace_id='ws-alice'"
        )
        .fetch_one(&pool)
        .await
        .unwrap(),
        0
    );
    // Simulate only the real retained-runtime ready receipt; the orchestrator
    // now selects its pre-created full pause, preserving both operation IDs.
    let resume = zuse_protocol::cloud_lifecycle::RuntimeRequest::Resumed {
        operation_id: op["wake_operation_id"].as_str().unwrap().into(),
        wake: None,
    };
    c.lifecycle_runtime("ws-alice", "fixture-alice-runtime", resume, now + 12000)
        .await
        .unwrap();
    c.lifecycle_runtime(
        "ws-alice",
        "fixture-alice-runtime",
        zuse_protocol::cloud_lifecycle::RuntimeRequest::Resumed {
            operation_id: op["wake_operation_id"].as_str().unwrap().into(),
            wake: None,
        },
        now + 12001,
    )
    .await
    .unwrap();
    let first_ready:i64=sqlx::query_scalar("SELECT (provider_effects->'timings'->>'ready')::bigint FROM zuse_rust_lifecycle_operations WHERE operation_id=$1").bind(op["wake_operation_id"].as_str()).fetch_one(&pool).await.unwrap();
    assert_eq!(
        first_ready,
        now + 12000,
        "duplicate ACK must preserve first readiness timing"
    );
    let wake_job: String = sqlx::query_scalar(
        "SELECT state FROM zuse_rust_jobs WHERE kind=$1 AND payload->>'operationId'=$2",
    )
    .bind(lifecycle::JOB_KIND)
    .bind(op["wake_operation_id"].as_str())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        wake_job, "completed",
        "new Ready ACK atomically completes its matching wake job"
    );
    // Preserve the mixed-version rollout regression explicitly: an older API
    // could persist Ready while leaving that original job pending. The current
    // orchestrator must still wait rather than replace its lifecycle pointer.
    sqlx::query("UPDATE zuse_rust_jobs SET state='pending',lease_token=NULL,lease_expires_at=NULL,due_at=0 WHERE kind=$1 AND payload->>'operationId'=$2").bind(lifecycle::JOB_KIND).bind(op["wake_operation_id"].as_str()).execute(&pool).await.unwrap();
    c.recreation_once(now + 13000).await.unwrap();
    let still_waking:String=sqlx::query_scalar("SELECT request_config->'rustRecreation'->>'phase' FROM api_cloud_workspaces WHERE workspace_id='ws-alice'").fetch_one(&pool).await.unwrap();
    assert_eq!(
        still_waking, "waking",
        "do not fence a pending original wake job"
    );
    assert!(
        c.lifecycle_once(&NoLifecycleEffects, now + 13001)
            .await
            .unwrap(),
        "canonical worker settles the Ready receipt without replaying wake/launch"
    );
    c.recreation_once(now + 15000).await.unwrap();
    let rejected = c
        .lifecycle_runtime(
            "ws-alice",
            "fixture-alice-runtime",
            zuse_protocol::cloud_lifecycle::RuntimeRequest::Parked {
                operation_id: op["pause_operation_id"].as_str().unwrap().into(),
            },
            now + 15001,
        )
        .await
        .unwrap_err();
    assert_eq!(rejected.code, "cloud_memory_park_receipt_rejected");
    c.lifecycle_runtime(
        "ws-alice",
        "fixture-alice-runtime",
        zuse_protocol::cloud_lifecycle::RuntimeRequest::Quiesced {
            operation_id: op["pause_operation_id"].as_str().unwrap().into(),
        },
        now + 15002,
    )
    .await
    .unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM zuse_rust_recovery_operations WHERE workspace_id='ws-alice'"
        )
        .fetch_one(&pool)
        .await
        .unwrap(),
        0,
        "full guest quiescence still needs original provider sleep completion before capture"
    );
    sqlx::raw_sql("CREATE TABLE api_github_enrollments(organization_id text,account_id text,installation_id bigint,data jsonb);CREATE TABLE api_github_join_policies(organization_id text,installation_id bigint,data jsonb)").execute(&pool).await.unwrap();
    // Even a delayed operation admitted by another current task author must
    // re-check its OWN requesting member before any subsequent work.
    sqlx::query("UPDATE api_cloud_workspaces SET account_id='organization:recreate-team',request_config=request_config||$1 WHERE workspace_id='ws-alice'").bind(json!({"rustActorId":"original-author","rustOrganizationExecutionVersion":1})).execute(&pool).await.unwrap();
    sqlx::query("UPDATE zuse_rust_computer_recreations SET owner_id='organization:recreate-team' WHERE operation_id=$1").bind(asleep["operationId"].as_str()).execute(&pool).await.unwrap();
    let departed = Arc::new(DepartedRecreator(std::sync::atomic::AtomicUsize::new(0)));
    c.configure_scope_authority(departed.clone(), None, vec!["recreate-team".into()])
        .unwrap();
    c.recreation_once(now + 18000).await.unwrap();
    assert_eq!(departed.0.load(std::sync::atomic::Ordering::SeqCst), 1);
    let denied: Value = sqlx::query_scalar(
        "SELECT to_jsonb(w) FROM api_cloud_workspaces w WHERE workspace_id='ws-alice'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(denied["status_code"], "organization_access_denied");
    assert!(leases_blocked(&denied));
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM zuse_rust_recovery_operations WHERE workspace_id='ws-alice'"
        )
        .fetch_one(&pool)
        .await
        .unwrap(),
        0
    );
    pool.close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .unwrap();
    admin.close().await;
}

struct LostRetirement {
    pool: PgPool,
    calls: std::sync::atomic::AtomicUsize,
    completed: std::sync::atomic::AtomicBool,
}
#[async_trait::async_trait]
impl recovery::RecoveryProvider for LostRetirement {
    async fn inspect(
        &self,
        id: &str,
    ) -> Result<Option<crate::providers::Machine>, crate::providers::ProviderError> {
        assert_eq!(id, "machine-bob");
        if self.completed.load(std::sync::atomic::Ordering::SeqCst) {
            return Ok(None);
        }
        Ok(Some(crate::providers::Machine {
            id: id.into(),
            name: "fixture".into(),
            state: crate::providers::MachineState::Paused,
            vcpu: None,
            memory_bytes: None,
        }))
    }
    async fn wake(
        &self,
        _: &str,
        _: &crate::providers::MachineState,
    ) -> Result<(), crate::providers::ProviderError> {
        panic!("retirement cannot wake")
    }
    async fn capture(
        &self,
        _: &recovery::CapturePlan,
    ) -> Result<(), crate::providers::ProviderError> {
        panic!("already verified")
    }
    async fn stop_capture(&self, _: &str, _: &str) -> Result<(), crate::providers::ProviderError> {
        panic!("no helper effects")
    }
    async fn pause(&self, _: &str) -> Result<(), crate::providers::ProviderError> {
        panic!("already paused")
    }
    async fn retire(&self, _: &str) -> Result<(), crate::providers::ProviderError> {
        panic!("must persist original retirement")
    }
    async fn retire_receipt(
        &self,
        id: &str,
    ) -> Result<crate::providers::contract::ProviderOperation, crate::providers::ProviderError>
    {
        assert_eq!(id, "machine-bob");
        tokio::time::timeout(
            Duration::from_secs(1),
            sqlx::query(
                "UPDATE api_cloud_workspaces SET revision=revision+1 WHERE workspace_id='ws-bob'",
            )
            .execute(&self.pool),
        )
        .await
        .expect("provider effect never holds workspace lock")
        .unwrap();
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Err(crate::providers::ProviderError::OutcomeUnknown)
    }
    async fn retirement_state(
        &self,
        r: &crate::providers::contract::ProviderOperation,
    ) -> Result<crate::providers::contract::OperationState, crate::providers::ProviderError> {
        assert_eq!(r.resource_id, "machine-bob");
        assert_eq!(r.desired, crate::providers::contract::DesiredState::Deleted);
        assert!(r.operation_id.is_none());
        Ok(
            if self.completed.load(std::sync::atomic::Ordering::SeqCst) {
                crate::providers::contract::OperationState::Completed
            } else {
                crate::providers::contract::OperationState::Pending
            },
        )
    }
}

#[test]
fn computer_recreation_environment_receipt_requires_every_bound_identity() {
    let request = json!({"version":1,"operationId":"recreate","archiveOperationId":"archive","workspaceId":"workspace","ownerId":"owner","generation":2,"storageIncarnationId":"disk","archiveSha256":"a".repeat(64)});
    let mut receipt = request.clone();
    receipt["status"] = json!("prepared");
    receipt["environmentReapply"] = json!(true);
    assert!(environment::receipt_matches(&request, &receipt));
    for field in [
        "operationId",
        "archiveOperationId",
        "workspaceId",
        "ownerId",
        "generation",
        "storageIncarnationId",
        "archiveSha256",
    ] {
        let mut wrong = receipt.clone();
        wrong[field] = json!("different");
        assert!(!environment::receipt_matches(&request, &wrong));
        let mut missing = receipt.clone();
        missing.as_object_mut().unwrap().remove(field);
        assert!(!environment::receipt_matches(&request, &missing));
    }
    receipt["environmentReapply"] = json!(false);
    assert!(!environment::receipt_matches(&request, &receipt));
}

struct NoLifecycleEffects;
#[async_trait::async_trait]
impl lifecycle::RetainedProvider for NoLifecycleEffects {
    async fn inspect_retained(
        &self,
        _: &str,
    ) -> Result<Option<crate::providers::Machine>, crate::providers::ProviderError> {
        panic!("terminal resume must not inspect/launch again")
    }
    async fn pause_retained(&self, _: &str) -> Result<(), crate::providers::ProviderError> {
        panic!("terminal wake")
    }
    async fn wake_retained(
        &self,
        _: &str,
        _: &crate::providers::MachineState,
    ) -> Result<(), crate::providers::ProviderError> {
        panic!("terminal wake")
    }
    async fn launch_retained(
        &self,
        _: &zuse_protocol::cloud_lifecycle::Directive,
        _: &str,
        _: &str,
    ) -> Result<(), crate::providers::ProviderError> {
        panic!("terminal wake")
    }
}
struct DepartedRecreator(std::sync::atomic::AtomicUsize);
#[async_trait::async_trait]
impl crate::auth::Identity for DepartedRecreator {
    async fn verify(&self, _: &str) -> Result<Principal, ApiError> {
        unreachable!()
    }
    async fn membership(
        &self,
        actor: &str,
        organization: &str,
    ) -> Result<crate::auth::Membership, ApiError> {
        assert_eq!(
            actor, "user-alice",
            "the requesting actor cannot borrow the task author's identity"
        );
        assert_eq!(organization, "recreate-team");
        self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Err(ApiError::forbidden("organization_access_denied"))
    }
}
