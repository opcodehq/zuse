use super::*;
use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};
use worker::*;
use zuse_protocol::cloud_computers::{EnvironmentRecipe, ProviderId, SaveEnvironment};

struct Auth;
#[async_trait::async_trait]
impl Authorization for Auth {
    async fn authorize(&self, _: &Request) -> Result<(), ApiError> {
        Ok(())
    }
}
struct Backend {
    plan: PreparationPlan,
    calls: AtomicUsize,
    uncertain: std::sync::atomic::AtomicBool,
}
#[async_trait::async_trait]
impl PreparationBackend for Backend {
    async fn resolve(&self, _: &Request) -> Result<Vec<PreparationPlan>, ApiError> {
        Ok(vec![self.plan.clone()])
    }
    async fn step(&self, operation: &Operation, mode: StepMode) -> Result<StepResult, ApiError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.uncertain.swap(false, Ordering::SeqCst) {
            assert!(mode == StepMode::Begin);
            return Err(ApiError::unavailable("test_lost_original_response"));
        }
        if operation.phase == "cleanup" {
            let mut resources = operation.resources["provider"].clone();
            if !resources.is_object() {
                resources = json!({});
            }
            resources["builderAbsent"] = json!(true);
            resources["verifierAbsent"] = json!(true);
            resources["unpublishedArtifactAbsent"] = json!(true);
            return Ok(StepResult::Advance {
                phase: "complete".into(),
                resources,
            });
        }
        if operation.phase != "publish" {
            if operation.phase == "allocate" {
                assert!(mode == StepMode::Reconcile);
            }
            let next = match operation.phase.as_str() {
                "allocate" => "prepare",
                "prepare" => "quiesce",
                "quiesce" => "capture",
                "capture" => "retire-builder",
                "retire-builder" => "verify",
                "verify" => "retire-verifier",
                "retire-verifier" => "publish",
                _ => panic!("unexpected phase"),
            };
            return Ok(StepResult::Advance {
                phase: next.into(),
                resources: json!({"builderAbsent":true,"verifierAbsent":true}),
            });
        }
        let identity = &operation.plan.identity;
        Ok(StepResult::Ready {
            artifact_id: "prepared-artifact".into(),
            artifact_version: None,
            artifact_binding: None,
            receipt: PreparationReceipt {
                version: 1,
                cache_key: identity.key()?,
                operation_id: operation.operation_id.clone(),
                runtime_sha256: identity.base_runtime_sha256.clone(),
                recipe_digest: identity.recipe_digest.clone(),
                dependency_digest: identity.dependency_digest(),
                source_commit: identity.source_commit.clone(),
                source_bundle_sha256: "e".repeat(64),
                credentials_removed: true,
                task_storage_absent: true,
                agent_profiles_absent: true,
                secret_free_source: true,
            },
            resources: json!({"artifactId":"prepared-artifact","builderAbsent":true,"verifierAbsent":true}),
        })
    }
}
#[tokio::test]
#[ignore = "requires explicit isolated localhost PostgreSQL"]
async fn preparation_is_project_scoped_reconciled_and_cache_hits_require_certification() {
    let url = std::env::var("ZUSE_CLOUD_TEST_DATABASE_URL").unwrap();
    let parsed = reqwest::Url::parse(&url).unwrap();
    assert!(matches!(
        parsed.host_str(),
        Some("127.0.0.1" | "localhost" | "::1")
    ));
    let admin = PgPool::connect(&url).await.unwrap();
    let schema = format!("cache_{}", uuid::Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&admin)
        .await
        .unwrap();
    let name = schema.clone();
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(4)
        .after_connect(move |c, _| {
            let name = name.clone();
            Box::pin(async move {
                sqlx::query(&format!("SET search_path={name}"))
                    .execute(c)
                    .await?;
                Ok(())
            })
        })
        .connect(&url)
        .await
        .unwrap();
    for script in [
        include_str!("../../tests/fixtures/legacy-catalog.sql"),
        include_str!("../../tests/fixtures/projects.sql"),
        include_str!("../../migrations/0009_computers.sql"),
        include_str!("../../migrations/0023_managed_base_admission_lock.sql"),
        MIGRATION,
    ] {
        sqlx::raw_sql(script).execute(&pool).await.unwrap();
    }
    sqlx::raw_sql("CREATE TABLE api_cloud_billing_periods(period_id text PRIMARY KEY,account_id text,status text,currency text,period_start bigint,period_end bigint,included_provider_cost_micros bigint,markup_basis_points bigint,overage_cap_micros bigint); CREATE TABLE api_cloud_billing_ledger(period_id text,kind text,amount_micros bigint); CREATE TABLE api_cloud_billing_reservations(period_id text,provider_cost_micros bigint,expires_at bigint); INSERT INTO api_cloud_billing_periods VALUES('period','user-alice','manual','USD',0,9999999999999,0,0,0);").execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO cloud_managed_bases VALUES('boxd','base-v1','managed-artifact','x86_64','{}','verified',0)").execute(&pool).await.unwrap();
    sqlx::query("ALTER TABLE cloud_managed_bases ADD COLUMN runtime_manifest jsonb")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE cloud_managed_bases SET runtime_manifest=$1").bind(json!({"initialRuntimeBootstrap":{"version":1,"mode":"stopped-memory-snapshot-v1","runtimeSha256":"a".repeat(64),"supervisorSha256":"b".repeat(64),"serviceSha256":"c".repeat(64),"manifestSha256":"d".repeat(64),"helperSha256":"e".repeat(64)}})).execute(&pool).await.unwrap();
    sqlx::raw_sql("INSERT INTO api_cloud_projects(project_id,account_id,repository_identity,repository_url,display_name,default_branch,visibility,git_connection_kind,configuration_digest,state,idempotency_key,created_at,updated_at) VALUES('project','user-alice','github.com/example/app','https://github.com/example/app.git','app','main','public','github-app','digest','connected','initial',0,0)").execute(&pool).await.unwrap();
    let setup =
        crate::computers::ComputerControl::from_pool(pool.clone(), vec!["user-alice".into()]);
    let recipe = EnvironmentRecipe::default();
    setup
        .save_environment(
            "user-alice",
            "project",
            SaveEnvironment {
                expected_revision: 0,
                recipe: recipe.clone(),
            },
            100,
        )
        .await
        .unwrap();
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM zuse_rust_project_preparation_requests")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(count, 1);
    let digest: String = sqlx::query_scalar("SELECT digest FROM cloud_environment_versions")
        .fetch_one(&pool)
        .await
        .unwrap();
    let identity = CacheIdentity {
        version: 1,
        owner_id: "user-alice".into(),
        project_id: "project".into(),
        repository_identity: "github.com/example/app".into(),
        provider: ProviderId::Boxd,
        architecture: "x86_64".into(),
        base_release_id: "base-v1".into(),
        base_artifact_id: "managed-artifact".into(),
        base_artifact_version: None,
        base_runtime_sha256: "a".repeat(64),
        recipe_source: "project".into(),
        recipe_digest: digest,
        source_commit: "c".repeat(40),
        dependency_inputs: BTreeMap::new(),
        staging_pilot_sha256: None,
    };
    let cap = CacheCapabilityReceipt {
        version: 1,
        runtime_sha256: identity.base_runtime_sha256.clone(),
        environment_cache_version: 1,
        secret_free_source_bundle_version: 1,
        retained_task_storage_absent_required: true,
        credential_free_preparation_required: true,
        preparation_capture_version: 1,
    };
    sqlx::query("INSERT INTO cloud_project_cache_capabilities VALUES('boxd','base-v1',$1,'x86_64',$2,'certified',0)").bind(&identity.base_runtime_sha256).bind(json!(cap)).execute(&pool).await.unwrap();
    let backend = Arc::new(Backend {
        plan: PreparationPlan {
            identity: identity.clone(),
            recipe: recipe.clone(),
            base_artifact_binding: None,
            initial_runtime_bootstrap: Some(
                crate::managed_bases::bootstrap::InitialRuntimeBootstrap {
                    version: 1,
                    mode: "stopped-memory-snapshot-v1".into(),
                    runtime_sha256: "a".repeat(64),
                    supervisor_sha256: "b".repeat(64),
                    service_sha256: "c".repeat(64),
                    manifest_sha256: "d".repeat(64),
                    helper_sha256: "e".repeat(64),
                },
            ),
            source_bundle_sha256: "e".repeat(64),
        },
        calls: AtomicUsize::new(0),
        uncertain: std::sync::atomic::AtomicBool::new(true),
    });
    let control =
        CacheControl::from_pool(pool.clone()).with_backend(Arc::new(Auth), backend.clone());
    assert!(control.work_once(101).await.unwrap());
    assert!(control.work_once(2_000).await.unwrap());
    let state: String = sqlx::query_scalar("SELECT state FROM zuse_rust_project_cache_operations")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(state, "uncertain");
    for now in [
        10_000, 20_000, 30_000, 40_000, 50_000, 60_000, 70_000, 80_000,
    ] {
        assert!(control.work_once(now).await.unwrap());
    }
    assert_eq!(backend.calls.load(Ordering::SeqCst), 9);
    let state: String =
        sqlx::query_scalar("SELECT state FROM zuse_rust_project_preparation_requests")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(state, "ready");
    let mut tx = pool.begin().await.unwrap();
    let hit = lookup_tx(&mut tx, &identity).await.unwrap().unwrap();
    assert_eq!(hit.artifact_id, "prepared-artifact");
    tx.commit().await.unwrap();
    // The actual application role can fence certification revocation without
    // receiving UPDATE on either operator registry.
    let role = format!("cache_reader_{}", uuid::Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE ROLE {role} NOLOGIN"))
        .execute(&admin)
        .await
        .unwrap();
    sqlx::raw_sql(&format!("GRANT USAGE ON SCHEMA {schema} TO {role}; GRANT SELECT ON {schema}.cloud_managed_bases,{schema}.cloud_project_cache_capabilities,{schema}.zuse_rust_project_cache_entries TO {role}; GRANT UPDATE ON {schema}.zuse_rust_project_cache_entries TO {role}; GRANT EXECUTE ON FUNCTION {schema}.zuse_rust_managed_base_admission_lock(text,text),{schema}.zuse_rust_project_cache_capability_lock(text,text) TO {role};")).execute(&admin).await.unwrap();
    let mut read = pool.begin().await.unwrap();
    sqlx::query(&format!("SET LOCAL ROLE {role}"))
        .execute(&mut *read)
        .await
        .unwrap();
    assert!(lookup_tx(&mut read, &identity).await.unwrap().is_some());
    let p = pool.clone();
    let retirement = tokio::spawn(async move {
        sqlx::query("UPDATE cloud_project_cache_capabilities SET state='retired'")
            .execute(&p)
            .await
            .unwrap()
    });
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    assert!(!retirement.is_finished());
    read.commit().await.unwrap();
    retirement.await.unwrap();
    let mut denied = pool.begin().await.unwrap();
    sqlx::query(&format!("SET LOCAL ROLE {role}"))
        .execute(&mut *denied)
        .await
        .unwrap();
    let error = sqlx::query("UPDATE cloud_project_cache_capabilities SET state='certified'")
        .execute(&mut *denied)
        .await
        .unwrap_err();
    assert_eq!(
        error.as_database_error().and_then(|e| e.code()).as_deref(),
        Some("42501")
    );
    denied.rollback().await.unwrap();
    sqlx::raw_sql(&format!("DROP OWNED BY {role}; DROP ROLE {role}"))
        .execute(&admin)
        .await
        .unwrap();
    sqlx::query("UPDATE cloud_project_cache_capabilities SET state='retired'")
        .execute(&pool)
        .await
        .unwrap();
    let mut tx = pool.begin().await.unwrap();
    assert!(lookup_tx(&mut tx, &identity).await.unwrap().is_none());
    tx.rollback().await.unwrap();
    // Retirement between admission and the first provider attempt restores the
    // original base. A creating/uncertain operation keeps its original source.
    sqlx::raw_sql("CREATE TABLE zuse_rust_allocations(operation_id text PRIMARY KEY,workspace_id text,generation bigint,snapshot_id text,machine_id text,phase text); INSERT INTO zuse_rust_allocations VALUES('allocation','ws-alice',1,'prepared-artifact',NULL,'creating');").execute(&pool).await.unwrap();
    let config = json!({"runtimeGeneration":1,"rustProjectCache":hit.config(),"repositoryCache":"project-environment-v1"});
    sqlx::query("UPDATE api_cloud_workspaces SET request_config=$1 WHERE workspace_id='ws-alice'")
        .bind(config)
        .execute(&pool)
        .await
        .unwrap();
    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD as B64};
    let key = ed25519_dalek::SigningKey::from_bytes(&[39; 32]);
    let jwk = json!({"kty":"OKP","crv":"Ed25519","d":B64.encode([39;32]),"x":B64.encode(key.verifying_key().as_bytes())});
    let runtime = crate::runtime::RuntimeControl::from_pool(
        pool.clone(),
        crate::crypto::MasterKey::parse(&B64.encode([38; 32])).unwrap(),
        crate::tickets::TicketSigner::from_jwk(&jwk.to_string(), "https://test.invalid").unwrap(),
    )
    .with_creation_accounts(vec!["user-alice".into()])
    .with_project_preparation(Arc::new(Auth), backend.clone());
    super::allocation_ref_tests::exercise(
        &pool,
        &runtime,
        &identity,
        &json!(recipe),
        &hit.config(),
    )
    .await;
    assert!(
        !control.retire_one(75_000).await.unwrap(),
        "a referenced cache cannot be deleted"
    );
    let request_id:String=sqlx::query_scalar("SELECT request_id FROM zuse_rust_project_preparation_requests WHERE environment_revision=1").fetch_one(&pool).await.unwrap();
    let signer =
        crate::tickets::TicketSigner::from_jwk(&jwk.to_string(), "https://test.invalid").unwrap();
    let authority_now = chrono::Utc::now().timestamp_millis();
    let proof = signer
        .allocation_scope_proof(
            &format!("preparation:{request_id}"),
            "project:project",
            1,
            authority_now,
        )
        .unwrap();
    runtime
        .authorize_project_preparation("project", &request_id, &proof, authority_now)
        .await
        .unwrap();
    for bad in [
        "client-jwt".into(),
        signer
            .allocation_scope_proof("different-workspace", "project:project", 1, authority_now)
            .unwrap(),
        signer
            .allocation_scope_proof(
                &format!("preparation:{request_id}"),
                "project:other-project",
                1,
                authority_now,
            )
            .unwrap(),
        signer
            .allocation_scope_proof(
                &format!("preparation:{request_id}"),
                "project:project",
                2,
                authority_now,
            )
            .unwrap(),
    ] {
        assert!(
            runtime
                .authorize_project_preparation("project", &request_id, &bad, authority_now)
                .await
                .is_err()
        );
    }
    assert!(
        runtime
            .authorize_project_preparation("project", &request_id, &proof, authority_now + 11000)
            .await
            .is_err()
    );
    super::private_source_tests::exercise(&pool, &runtime, &signer, &request_id).await;
    let mut workspace: Value = sqlx::query_scalar(
        "SELECT to_jsonb(w) FROM api_cloud_workspaces w WHERE workspace_id='ws-alice'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let mut allocation: Value =
        sqlx::query_scalar("SELECT to_jsonb(a) FROM zuse_rust_allocations a")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(
        !runtime
            .project_cache_allocation_source(&mut workspace, &mut allocation)
            .await
            .unwrap()
    );
    assert_eq!(allocation["snapshot_id"], "prepared-artifact");
    sqlx::query("UPDATE zuse_rust_allocations SET phase='planned'")
        .execute(&pool)
        .await
        .unwrap();
    allocation["phase"] = json!("planned");
    assert!(
        runtime
            .project_cache_allocation_source(&mut workspace, &mut allocation)
            .await
            .unwrap()
    );
    assert_eq!(allocation["snapshot_id"], "managed-artifact");
    assert!(workspace["request_config"]["rustProjectCache"].is_null());
    sqlx::query("UPDATE cloud_project_cache_capabilities SET state='certified'")
        .execute(&pool)
        .await
        .unwrap();
    // Credential-independent recipes reuse the exact compatible artifact. A new
    // recipe version queues only this project and cannot replace the prior cache.
    setup
        .save_environment(
            "user-alice",
            "project",
            SaveEnvironment {
                expected_revision: 1,
                recipe,
            },
            80_000,
        )
        .await
        .unwrap();
    let preserved: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM zuse_rust_project_cache_entries WHERE state='ready'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(preserved, 1);
    sqlx::query("UPDATE api_cloud_billing_periods SET status='grace'")
        .execute(&pool)
        .await
        .unwrap();
    assert!(control.work_once(90_000).await.unwrap());
    let error:String=sqlx::query_scalar("SELECT last_error_code FROM zuse_rust_project_preparation_requests ORDER BY environment_revision DESC LIMIT 1").fetch_one(&pool).await.unwrap();
    assert_eq!(error, "cloud_billing_hold");
    assert_eq!(backend.calls.load(Ordering::SeqCst), 9);
    sqlx::query("DELETE FROM api_entitlements WHERE account_id='user-alice'")
        .execute(&pool)
        .await
        .unwrap();
    assert!(control.work_once(130_000).await.unwrap());
    let error:String=sqlx::query_scalar("SELECT last_error_code FROM zuse_rust_project_preparation_requests ORDER BY environment_revision DESC LIMIT 1").fetch_one(&pool).await.unwrap();
    assert_eq!(error, "cloud_entitlement_required");
    assert_eq!(backend.calls.load(Ordering::SeqCst), 9);
    sqlx::query("UPDATE cloud_project_cache_capabilities SET state='retired'")
        .execute(&pool)
        .await
        .unwrap();
    assert!(control.retire_one(140_000).await.unwrap());
    let retired: String =
        sqlx::query_scalar("SELECT state FROM zuse_rust_project_cache_entries WHERE cache_key=$1")
            .bind(identity.key().unwrap())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(retired, "retired");
    let cleanup = CacheControl::from_pool(pool.clone());
    cleanup
        .install_mode(Arc::new(Auth), backend.clone(), false)
        .unwrap();
    assert!(cleanup.work_once(150_000).await.unwrap());
    assert!(
        !cleanup.work_once(160_000).await.unwrap(),
        "rollback cannot start queued preparations"
    );
    assert_eq!(backend.calls.load(Ordering::SeqCst), 10);
    let retired: String = sqlx::query_scalar(
        "SELECT state FROM zuse_rust_project_cache_operations WHERE cache_key=$1",
    )
    .bind(identity.key().unwrap())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(retired, "deleted");
    let observed:(String,Option<i64>,String)=sqlx::query_as("SELECT resource_id,absent_observed_at,cost_status FROM zuse_rust_project_cache_resources WHERE resource_kind='artifact'").fetch_one(&pool).await.unwrap();
    assert_eq!(observed.0, "prepared-artifact");
    assert!(observed.1.is_some());
    assert_eq!(observed.2, "unpriced");
    // An uncertain original create remains cleanup-authorized after payment
    // withdrawal. No new allocation/setup step can run to recover its receipt.
    sqlx::query("UPDATE cloud_project_cache_capabilities SET state='certified'")
        .execute(&pool)
        .await
        .unwrap();
    let mut lost_plan = backend.plan.clone();
    lost_plan.identity.source_commit = "d".repeat(40);
    let lost_key = lost_plan.identity.key().unwrap();
    sqlx::query("INSERT INTO zuse_rust_project_preparation_requests(request_id,owner_id,actor_id,project_id,environment_revision,recipe_digest,repository_identity,requested_ref,state,next_attempt_at,created_at,updated_at) VALUES('prep-lost','user-alice','user-alice','project',2,$1,'github.com/example/app',$2,'preparing',200000,200000,200000)").bind(&lost_plan.identity.recipe_digest).bind(&lost_plan.identity.source_commit).execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO zuse_rust_project_cache_entries(cache_key,owner_id,project_id,provider,architecture,base_release_id,base_artifact_id,base_runtime_sha256,identity,recipe_digest,dependency_digest,source_commit,state,created_at) VALUES($1,'user-alice','project','boxd','x86_64','base-v1','managed-artifact',$2,$3,$4,$5,$6,'preparing',200000)").bind(&lost_key).bind(&lost_plan.identity.base_runtime_sha256).bind(json!(lost_plan.identity)).bind(&lost_plan.identity.recipe_digest).bind(lost_plan.identity.dependency_digest()).bind(&lost_plan.identity.source_commit).execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO zuse_rust_project_cache_operations(operation_id,request_id,cache_key,owner_id,actor_id,project_id,provider,state,phase,plan,resource_receipts,started_at,next_attempt_at,created_at,updated_at) VALUES('cache-lost','prep-lost',$1,'user-alice','user-alice','project','boxd','uncertain','allocate',$2,$3,200000,200000,200000,200000)").bind(&lost_key).bind(json!(lost_plan)).bind(json!({"effects":{"allocate":{"requested":true,"operationId":"cache-lost:allocate"}},"provider":{"builderId":"original-unconfirmed-builder"}})).execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO zuse_rust_project_preparation_keys VALUES('prep-lost',$1)")
        .bind(&lost_key)
        .execute(&pool)
        .await
        .unwrap();
    assert!(control.work_once(200_000).await.unwrap());
    let lost:(String,String,String)=sqlx::query_as("SELECT state,phase,last_error_code FROM zuse_rust_project_cache_operations WHERE operation_id='cache-lost'").fetch_one(&pool).await.unwrap();
    assert_eq!(
        lost,
        (
            "deleted".into(),
            "complete".into(),
            "cloud_entitlement_required".into()
        )
    );
    assert_eq!(
        backend.calls.load(Ordering::SeqCst),
        11,
        "only one original cleanup step, no allocation/setup replay"
    );
    let original:(String,bool)=sqlx::query_as("SELECT resource_id,absent_observed_at IS NOT NULL FROM zuse_rust_project_cache_resources WHERE operation_id='cache-lost' AND resource_kind='builder'").fetch_one(&pool).await.unwrap();
    assert_eq!(original, ("original-unconfirmed-builder".into(), true));
    // Optional-schema startup is harmless, but authorization must NEVER turn
    // missing schema or disabled preparation into permission to mint a proof.
    sqlx::query("DROP TABLE zuse_rust_project_cache_resources")
        .execute(&pool)
        .await
        .unwrap();
    let baseline = Arc::new(crate::runtime::RuntimeControl::from_pool(
        pool.clone(),
        crate::crypto::MasterKey::parse(&B64.encode([38; 32])).unwrap(),
        crate::tickets::TicketSigner::from_jwk(&jwk.to_string(), "https://test.invalid").unwrap(),
    ));
    assert_eq!(
        baseline
            .authorize_project_preparation("project", "missing", "invalid", authority_now)
            .await
            .unwrap_err()
            .code,
        "project_cache_schema_not_ready"
    );
    if std::env::var("ZUSE_CLOUD_PROJECT_PREPARATION_ENABLED").as_deref() != Ok("1") {
        baseline
            .configure_public_project_preparation()
            .await
            .unwrap();
    }
    pool.close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .unwrap();
    admin.close().await;
}

#[test]
fn cache_capability_is_required_only_for_explicit_project_artifacts() {
    require_runtime_capability(&json!({}), None).unwrap();
    require_runtime_capability(&json!({"rustProjectCache":null}), None).unwrap();
    assert_eq!(
        require_runtime_capability(
            &json!({"rustProjectCache":{"version":1}}),
            Some(&vec!["rust-rpc-v1".into()])
        )
        .unwrap_err()
        .code,
        "project_cache_runtime_update_required"
    );
    require_runtime_capability(
        &json!({"rustProjectCache":{"version":1}}),
        Some(&vec![zuse_cloud_runtime::project_cache::CAPABILITY.into()]),
    )
    .unwrap();
}

#[test]
fn preparation_capture_capability_cannot_be_inferred_from_legacy_receipts() {
    let runtime = "a".repeat(64);
    let legacy = json!({"version":1,"runtimeSha256":runtime,"environmentCacheVersion":1,"secretFreeSourceBundleVersion":1,"retainedTaskStorageAbsentRequired":true,"credentialFreePreparationRequired":true});
    let receipt: CacheCapabilityReceipt = serde_json::from_value(legacy.clone()).unwrap();
    assert!(!receipt.valid(&runtime));
    let mut current = legacy;
    current["preparationCaptureVersion"] = json!(1);
    assert!(
        serde_json::from_value::<CacheCapabilityReceipt>(current.clone())
            .unwrap()
            .valid(&runtime)
    );
    current["preparationCaptureVersion"] = json!(2);
    assert!(
        !serde_json::from_value::<CacheCapabilityReceipt>(current)
            .unwrap()
            .valid(&runtime)
    );
}
