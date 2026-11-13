use super::tests::release;
use super::*;
use crate::{
    access::{Access, Scope},
    auth::Principal,
    crypto::MasterKey,
    runtime::RuntimeControl,
    tickets::TicketSigner,
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use sha2::{Digest, Sha256};
use std::sync::Arc;

struct AllowanceProvider {
    pool: PgPool,
    release: String,
    mode: u8,
    machine: std::sync::Mutex<Option<crate::providers::Machine>>,
    creates: std::sync::atomic::AtomicUsize,
    launches: std::sync::atomic::AtomicUsize,
    launch_receipt: std::sync::Mutex<Option<Value>>,
}
#[async_trait::async_trait]
impl crate::runtime::allocation::AllocationProvider for AllowanceProvider {
    async fn find(
        &self,
        name: &str,
    ) -> Result<Option<crate::providers::Machine>, crate::providers::ProviderError> {
        Ok(self
            .machine
            .lock()
            .unwrap()
            .as_ref()
            .filter(|m| m.name == name || m.id == name)
            .cloned())
    }
    async fn create(
        &self,
        _: &str,
        name: &str,
    ) -> Result<crate::providers::Machine, crate::providers::ProviderError> {
        self.creates
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let machine = crate::providers::Machine {
            id: format!("allowance-resource-{}", self.mode),
            name: name.into(),
            state: crate::providers::MachineState::Running,
            vcpu: Some(1),
            memory_bytes: Some(4096),
        };
        *self.machine.lock().unwrap() = Some(machine.clone());
        if self.mode != 0 {
            sqlx::query("UPDATE cloud_managed_bases SET state='retired' WHERE release_id=$1")
                .bind(&self.release)
                .execute(&self.pool)
                .await
                .unwrap();
        }
        if self.mode == 2 {
            return Err(crate::providers::ProviderError::OutcomeUnknown);
        }
        Ok(machine)
    }
    async fn launch(
        &self,
        machine: &str,
        launch: &crate::runtime::allocation::RuntimeLaunch,
    ) -> Result<(), crate::providers::ProviderError> {
        // The provider contract is idempotent for the same launch. A guest that
        // has not enrolled yet can receive this exact delivery again.
        let identity = json!({"machine":machine,"operation":launch.operation_id,"workspace":launch.workspace_id,
            "generation":launch.generation,"storage":launch.incarnation,"bootSha256":format!("{:x}",Sha256::digest(launch.boot_token.as_bytes()))});
        let mut receipt = self.launch_receipt.lock().unwrap();
        if let Some(prior) = receipt.as_ref() {
            assert_eq!(
                prior, &identity,
                "retry changed its original launch binding"
            );
        } else {
            *receipt = Some(identity);
            self.launches
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
        Ok(())
    }
}
fn copy(
    body: &crate::runtime::creation::CreateWorkspace,
) -> crate::runtime::creation::CreateWorkspace {
    serde_json::from_value(serde_json::to_value(body).unwrap()).unwrap()
}

#[tokio::test]
#[ignore = "requires explicit isolated localhost PostgreSQL"]
async fn cache_miss_admission_pins_a_platform_release_without_customer_builds() {
    admission_case(false, false).await;
}

#[tokio::test]
#[ignore = "requires explicit isolated localhost PostgreSQL"]
async fn staging_owner_limit_preserves_existing_work_and_idempotent_admission() {
    admission_case(true, false).await;
}

#[tokio::test]
#[ignore = "requires explicit isolated localhost PostgreSQL"]
async fn deferred_repository_checks_admit_locally_and_still_fence_git_credentials() {
    admission_case(false, true).await;
}

#[tokio::test]
#[ignore = "requires explicit isolated localhost PostgreSQL"]
async fn ready_user_image_replaces_the_base_snapshot_with_base_fallback() {
    admission_case_with(false, false, true).await;
}

async fn admission_case(expanded_capacity_only: bool, defer_repository: bool) {
    admission_case_with(expanded_capacity_only, defer_repository, false).await;
}

/// A ready user image on `release`, as the builder publishes it.
async fn ready_user_image(
    pool: &PgPool,
    release: &ManagedBaseRelease,
    artifact: &str,
    built: i64,
) -> String {
    use crate::project_caches::user_images::*;
    let recipe = zuse_protocol::cloud_computers::MachineRecipe {
        setup_script: "npm i -g pnpm".into(),
        ..Default::default()
    };
    let id = format!("uimg_{}", uuid::Uuid::new_v4().simple());
    let bootstrap = release.initial_runtime_bootstrap.clone().unwrap();
    let plan = BuildPlan {
        version: 1,
        image_id: id.clone(),
        identity: UserImageIdentity {
            version: 1,
            owner_id: "user-test".into(),
            provider: ProviderId::Boxd,
            architecture: release.architecture.clone(),
            base_release_id: release.release_id.clone(),
            base_artifact_id: release.artifact_id.clone(),
            base_artifact_version: "1".into(),
            base_runtime_sha256: bootstrap.runtime_sha256.clone(),
            recipe_digest: image_digest(&recipe).unwrap(),
        },
        setup_script: recipe.setup_script.clone(),
        setup_timeout_seconds: recipe.setup_timeout_seconds,
        initial_runtime_bootstrap: bootstrap,
    };
    sqlx::query("INSERT INTO cloud_machine_images(image_id,owner_id,provider,reason,state,phase,recipe_revision,recipe_digest,architecture,base_release_id,base_artifact_id,base_artifact_version,base_runtime_sha256,identity_key,plan,artifact_id,artifact_version,next_attempt_at,started_at,built_at,created_at,updated_at) VALUES($1,'user-test','boxd','recipe','ready','complete',1,$2,$3,$4,$5,'1',$6,$7,$8,$9,'1',0,$10,$10,$10,$10)")
        .bind(&id).bind(&plan.identity.recipe_digest).bind(&plan.identity.architecture).bind(&plan.identity.base_release_id)
        .bind(&plan.identity.base_artifact_id).bind(&plan.identity.base_runtime_sha256).bind(plan.identity.key().unwrap())
        .bind(json!(plan)).bind(artifact).bind(built).execute(pool).await.unwrap();
    id
}

async fn admission_case_with(
    expanded_capacity_only: bool,
    defer_repository: bool,
    user_image: bool,
) {
    let url = std::env::var("ZUSE_CLOUD_TEST_DATABASE_URL").expect("isolated PostgreSQL required");
    let parsed = reqwest::Url::parse(&url).unwrap();
    assert!(matches!(
        parsed.host_str(),
        Some("127.0.0.1" | "localhost" | "::1")
    ));
    let admin = PgPool::connect(&url).await.unwrap();
    let schema = "zuse_staging".to_owned();
    let _staging = crate::STAGING_SCHEMA_LOCK.lock().await;
    sqlx::query(&format!("DROP SCHEMA IF EXISTS {schema} CASCADE"))
        .execute(&admin)
        .await
        .unwrap();
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
    for migration in [
        include_str!("../../migrations/0000_application.sql"),
        include_str!("../../migrations/0001_control_plane.sql"),
        include_str!("../../migrations/0002_gateway_leases.sql"),
        include_str!("../../migrations/0003_command_mailbox.sql"),
        include_str!("../../migrations/0004_allocations.sql"),
        include_str!("../../migrations/0005_lifecycle.sql"),
        include_str!("../../migrations/0006_recovery.sql"),
        include_str!("../../migrations/0007_images.sql"),
        include_str!("../../migrations/0007_restore.sql"),
        include_str!("../../migrations/0008_delete.sql"),
        include_str!("../../migrations/0008_launch_assets.sql"),
        include_str!("../../migrations/0009_computers.sql"),
        include_str!("../../migrations/0010_managed_bases.sql"),
        include_str!("../../migrations/0011_transcript_heads.sql"),
        include_str!("../../migrations/0012_provider_receipts.sql"),
        include_str!("../../migrations/0013_recovery_receipts.sql"),
        include_str!("../../migrations/0014_restore_receipts.sql"),
        include_str!("../../migrations/0015_github.sql"),
        include_str!("../../migrations/0016_task_history.sql"),
        include_str!("../../migrations/0019_workload_secrets.sql"),
        include_str!("../../migrations/0020_staging_managed_base_candidates.sql"),
        include_str!("../../migrations/0023_managed_base_admission_lock.sql"),
        include_str!("../../migrations/0025_admission_read_batch.sql"),
        include_str!("../../migrations/0031_machine_images.sql"),
        if user_image {
            crate::project_caches::user_images::MIGRATION
        } else {
            "SELECT 1"
        },
    ] {
        sqlx::raw_sql(migration).execute(&pool).await.unwrap();
    }
    // Use the actual provisioner role policy, including forbidden registry UPDATE.
    let database: String = sqlx::query_scalar("SELECT current_database()")
        .fetch_one(&admin)
        .await
        .unwrap();
    let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../infra/cloud/single-host/provision.py");
    let output=std::process::Command::new("python3").args(["-c","import importlib.util,sys;s=importlib.util.spec_from_file_location('provision',sys.argv[1]);m=importlib.util.module_from_spec(s);s.loader.exec_module(m);print(m.role_sql('staging',sys.argv[2],'synthetic-locked-role-password',True))"]).arg(script).arg(&database).output().unwrap();
    assert!(output.status.success());
    sqlx::raw_sql(std::str::from_utf8(&output.stdout).unwrap())
        .execute(&admin)
        .await
        .unwrap();
    let mut restricted_url = reqwest::Url::parse(&url).unwrap();
    restricted_url.set_username("zuse_cloud_staging").unwrap();
    restricted_url
        .set_password(Some("synthetic-locked-role-password"))
        .unwrap();
    let application = PgPool::connect(restricted_url.as_str()).await.unwrap();
    assert!(
        !sqlx::query_scalar::<_, bool>(
            "SELECT has_table_privilege(current_user,'cloud_managed_bases','UPDATE')"
        )
        .fetch_one(&application)
        .await
        .unwrap()
    );
    let denied = sqlx::query("SELECT provider FROM cloud_managed_bases WHERE false FOR SHARE")
        .execute(&application)
        .await
        .unwrap_err();
    assert_eq!(
        denied.as_database_error().unwrap().code().as_deref(),
        Some("42501")
    );
    let now = chrono::Utc::now().timestamp_millis();
    sqlx::query("INSERT INTO api_entitlements(entitlement_id,account_id,kind,provider,status,paid_through,created_at,updated_at) VALUES('ent','user-test','cloud-workspace','polar','active',$1,$2,$2)").bind(now+86400000).bind(now).execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO api_cloud_billing_periods(period_id,account_id,status,period_start,period_end,base_price_micros,included_provider_cost_micros,markup_basis_points,price_catalog_version,overage_cap_micros,created_at,updated_at) VALUES('period','user-test','manual',$1,$2,0,5000000,0,'fixture',0,$1,$1)").bind(now-1000).bind(now+86400000).execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO api_cloud_projects(project_id,account_id,repository_identity,repository_url,display_name,default_branch,visibility,git_connection_kind,configuration_digest,state,idempotency_key,created_at,updated_at) VALUES('project','user-test','github.com/example/repo','https://github.com/example/repo.git','repo','main','public','github-app','fixture','connected','project-key',$1,$1)").bind(now).execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO cloud_agent_connections(connection_id,owner_id,idempotency_key,request_digest,agent,method,state,created_at,updated_at) VALUES('connection','user-test','auth-key','fixture','codex','api-key','connected',$1,$1)").bind(now).execute(&pool).await.unwrap();
    let key = URL_SAFE_NO_PAD.encode([48; 32]);
    let signing = ed25519_dalek::SigningKey::from_bytes(&[49; 32]);
    let jwk = json!({"kty":"OKP","crv":"Ed25519","d":URL_SAFE_NO_PAD.encode([49;32]),"x":URL_SAFE_NO_PAD.encode(signing.verifying_key().as_bytes())});
    let authority = Arc::new(crate::connections::ConnectionControl::from_pool(
        application.clone(),
        MasterKey::parse(&key).unwrap(),
        vec!["user-test".into()],
    ));
    let control = RuntimeControl::from_pool(
        application.clone(),
        MasterKey::parse(&key).unwrap(),
        TicketSigner::from_jwk(&jwk.to_string(), "https://api.example.test").unwrap(),
    )
    .with_creation_accounts(vec!["user-test".into()])
    .with_fast_start_accounts(if defer_repository {
        vec!["user-test".into()]
    } else {
        vec![]
    })
    .with_credential_authority(authority.clone());
    let github =
        crate::github::native::admission_fixture::connected(&application, "user-test", "user-test")
            .await;
    let identity = Arc::new(crate::github::native::admission_fixture::PersonalIdentity);
    control
        .configure_scope_authority(identity.clone(), Some(github.github.clone()), vec![])
        .unwrap();
    assert!(control.admission_batch_available().await.unwrap());
    let mut store = crate::store::Store::from_pool(application.clone());
    store.connections = Some(authority);
    let access = Access {
        actor: Principal {
            account_id: "user-test".into(),
        },
        scope: Scope::Personal,
        owner_id: "user-test".into(),
        membership: None,
    };
    let body: crate::runtime::creation::CreateWorkspace = serde_json::from_value(json!({"projectId":"project","providerId":"boxd","baseRef":"cccccccccccccccccccccccccccccccccccccccc","branch":"task/new","agent":"codex","model":"fixture","agentConnectionId":"connection","firstMessage":"Test initial turn","idempotencyKey":"task-key"})).unwrap();
    assert_eq!(
        control
            .create_workspace(&access, copy(&body), now)
            .await
            .unwrap_err()
            .code,
        "managed_base_unavailable_for_agent"
    );
    let mut r = release();
    r.manifest.rust_github_credentials_version = Some(1);
    if user_image {
        r.initial_runtime_bootstrap =
            Some(crate::managed_bases::bootstrap::InitialRuntimeBootstrap {
                version: 1,
                mode: "stopped-memory-snapshot-v1".into(),
                runtime_sha256: "a".repeat(64),
                supervisor_sha256: "b".repeat(64),
                service_sha256: "c".repeat(64),
                manifest_sha256: "d".repeat(64),
                helper_sha256: "e".repeat(64),
                runtime_update_helper_sha256: None,
            });
    }
    r.register(&pool, now).await.unwrap();
    r.register(&pool, now + 1).await.unwrap();
    let mut held = application.begin().await.unwrap();
    admission_lock::record(&mut held, "boxd", &r.release_id)
        .await
        .unwrap()
        .unwrap();
    let mut revocation = pool.begin().await.unwrap();
    sqlx::query("SET LOCAL lock_timeout='80ms'")
        .execute(&mut *revocation)
        .await
        .unwrap();
    let conflict = sqlx::query(
        "UPDATE cloud_managed_bases SET state='retired' WHERE provider='boxd' AND release_id=$1",
    )
    .bind(&r.release_id)
    .execute(&mut *revocation)
    .await
    .unwrap_err();
    assert_eq!(
        conflict.as_database_error().unwrap().code().as_deref(),
        Some("55P03")
    );
    revocation.rollback().await.unwrap();
    held.rollback().await.unwrap();

    assert!(
        store
            .managed_launch_catalog(&[])
            .await
            .unwrap()
            .providers
            .is_empty()
    );
    let connections = store.agent_connections("user-test").await.unwrap();
    let catalog = store.managed_launch_catalog(&connections).await.unwrap();
    assert_eq!(catalog.providers.len(), 1);
    assert_eq!(catalog.providers[0].id, zuse_protocol::HarnessId::Codex);
    assert_eq!(catalog.providers[0].default_model, "fixture");
    let mut unknown_model = copy(&body);
    unknown_model.model = "uninstalled-model".into();
    assert_eq!(
        control
            .create_workspace(&access, unknown_model, now)
            .await
            .unwrap_err()
            .code,
        "managed_base_unavailable_for_agent"
    );
    let mut changed = r.clone();
    changed.artifact_id = "other-snapshot".into();
    assert_eq!(
        changed.register(&pool, now).await.unwrap_err().code,
        "managed_base_release_immutable"
    );
    let before_reads = github.reads.load(std::sync::atomic::Ordering::SeqCst);
    let image = if user_image {
        // A recipe without a setup script never uses an image, even a ready one.
        sqlx::query("INSERT INTO cloud_machine_versions(owner_id,revision,actor_id,recipe,digest,created_at) VALUES('user-test',1,'user-test','{\"instructions\":\"x\"}',$1,$2)").bind("f".repeat(64)).bind(now).execute(&pool).await.unwrap();
        let id = ready_user_image(&pool, &r, "user-image-snapshot", now).await;
        let mut tx = application.begin().await.unwrap();
        assert!(
            crate::project_caches::user_images::selection::for_restore_tx(
                &mut tx,
                "user-test",
                "boxd",
                &r.release_id
            )
            .await
            .unwrap()
            .is_none()
        );
        tx.rollback().await.unwrap();
        sqlx::query("INSERT INTO cloud_machine_versions(owner_id,revision,actor_id,recipe,digest,created_at) VALUES('user-test',2,'user-test','{\"setupScript\":\"npm i -g pnpm\"}',$1,$2)").bind("f".repeat(64)).bind(now).execute(&pool).await.unwrap();
        Some(id)
    } else {
        None
    };
    let (left, right) = tokio::join!(
        control.create_workspace(&access, copy(&body), now),
        control.create_workspace(&access, copy(&body), now)
    );
    let (created, left_new) = left.unwrap();
    let (repeated, right_new) = right.unwrap();
    assert_ne!(left_new, right_new);
    assert_eq!(
        created["workspace"]["workspaceId"],
        repeated["workspace"]["workspaceId"]
    );
    let id = created["workspace"]["workspaceId"].as_str().unwrap();
    let session = created["workspace"]["initialSessionId"].as_str().unwrap();
    let saved = control
        .task_history(&access, id, session, None)
        .await
        .unwrap();
    assert_eq!(saved["source"], "admission");
    assert_eq!(saved["admission"]["firstMessage"], "Test initial turn");
    assert!(
        saved.get("cursor").is_none(),
        "Admitting a command cannot invent a guest transcript cursor"
    );
    let cipher: String = sqlx::query_scalar(
        "SELECT ciphertext FROM zuse_rust_task_admissions WHERE workspace_id=$1",
    )
    .bind(id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(!cipher.contains("Test initial turn"));
    let stranger = Access {
        actor: Principal {
            account_id: "other-user".into(),
        },
        scope: Scope::Personal,
        owner_id: "other-user".into(),
        membership: None,
    };
    assert_eq!(
        control
            .task_history(&stranger, id, session, None)
            .await
            .unwrap_err()
            .code,
        "cloud_workspace_not_found"
    );
    let row: Value =
        sqlx::query_scalar("SELECT to_jsonb(w) FROM api_cloud_workspaces w WHERE workspace_id=$1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
    // Optional0028 is deliberately absent: even an exact-SHA request must
    // create from the managed base using the actual restricted application role.
    let cache_schema: bool =
        sqlx::query_scalar("SELECT to_regclass('zuse_rust_project_cache_entries') IS NOT NULL")
            .fetch_one(&application)
            .await
            .unwrap();
    assert!(!cache_schema);
    assert!(!control.project_cache_control.available().await.unwrap());
    assert!(row["request_config"]["rustProjectCache"].is_null());
    assert!(row["build_id"].is_null());
    assert_eq!(row["managed_release_id"], r.release_id);
    assert_eq!(
        row["request_config"]["rustRepositoryCheckout"]["fetchRemote"],
        true
    );
    assert!(row["request_config"]["rustRepositoryCheckout"]["cachePath"].is_null());
    assert_eq!(row["request_config"]["repositoryCache"], "none");
    let counts:(i64,i64,i64) = sqlx::query_as("SELECT (SELECT count(*) FROM api_cloud_project_builds),(SELECT count(*) FROM api_cloud_snapshots),(SELECT count(*) FROM zuse_rust_allocations)").fetch_one(&pool).await.unwrap();
    assert_eq!(counts, (0, 0, 1));
    if let Some(image) = image {
        let snapshot: String = sqlx::query_scalar(
            "SELECT snapshot_id FROM zuse_rust_allocations WHERE workspace_id=$1",
        )
        .bind(id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(snapshot, "user-image-snapshot");
        assert_eq!(row["request_config"]["rustMachineImage"]["imageId"], image);
        // Launch verification is unchanged: the base's own pins.
        assert_eq!(
            row["request_config"]["initialRuntimeBootstrap"],
            json!(r.initial_runtime_bootstrap)
        );
        let mut tx = application.begin().await.unwrap();
        let restore = crate::project_caches::user_images::selection::for_restore_tx(
            &mut tx,
            "user-test",
            "boxd",
            &r.release_id,
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(restore.image_id, image);
        tx.rollback().await.unwrap();
        // A newer base is registered: the image on the older, still verified
        // base keeps serving until its rebuild verifies.
        let mut newer = r.clone();
        newer.release_id = "managed-fixture-v2".into();
        newer.manifest.build_id = newer.release_id.clone();
        newer.artifact_id = "platform-snapshot-v2".into();
        newer.register(&pool, now + 10).await.unwrap();
        let next = |key: &str, branch: &str| {
            let mut next = copy(&body);
            next.idempotency_key = key.into();
            next.branch = Some(branch.into());
            next
        };
        let (second, _) = control
            .create_workspace(&access, next("task-key-2", "task/two"), now + 20)
            .await
            .unwrap();
        let second: Value = sqlx::query_scalar(
            "SELECT to_jsonb(w) FROM api_cloud_workspaces w WHERE workspace_id=$1",
        )
        .bind(second["workspace"]["workspaceId"].as_str())
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(second["managed_release_id"], r.release_id);
        assert_eq!(
            second["request_config"]["rustMachineImage"]["imageId"],
            image
        );
        // Its base retired: new computers fall back to the newest base.
        sqlx::query("UPDATE cloud_managed_bases SET state='retired' WHERE release_id=$1")
            .bind(&r.release_id)
            .execute(&pool)
            .await
            .unwrap();
        let (third, _) = control
            .create_workspace(&access, next("task-key-3", "task/three"), now + 30)
            .await
            .unwrap();
        let third: Value = sqlx::query_scalar(
            "SELECT to_jsonb(w) FROM api_cloud_workspaces w WHERE workspace_id=$1",
        )
        .bind(third["workspace"]["workspaceId"].as_str())
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(third["managed_release_id"], "managed-fixture-v2");
        assert!(third["request_config"]["rustMachineImage"].is_null());
        let snapshot: String = sqlx::query_scalar(
            "SELECT snapshot_id FROM zuse_rust_allocations WHERE workspace_id=$1",
        )
        .bind(third["workspace_id"].as_str())
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(snapshot, "platform-snapshot-v2");
        application.close().await;
        pool.close().await;
        sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
            .execute(&admin)
            .await
            .unwrap();
        sqlx::query("DROP OWNED BY zuse_cloud_staging")
            .execute(&admin)
            .await
            .unwrap();
        sqlx::query("DROP ROLE zuse_cloud_staging")
            .execute(&admin)
            .await
            .unwrap();
        return;
    }

    if defer_repository {
        assert_eq!(
            github.reads.load(std::sync::atomic::Ordering::SeqCst),
            before_reads,
            "admission must not wait for GitHub HTTP"
        );
        assert_eq!(
            row["request_config"]["rustRepositoryAuthorizationMode"],
            "preparation-v1"
        );
        assert_eq!(row["request_config"]["rustGithubUserId"], 42);
        // The same task's later Git grant must observe current repository policy.
        sqlx::query("UPDATE zuse_rust_github_installations SET allowed_repositories='[]'::jsonb WHERE owner_id='user-test'").execute(&pool).await.unwrap();
        assert_eq!(
            github
                .github
                .scoped_git("user-test", "user-test", "example/repo")
                .await
                .err()
                .unwrap()
                .code,
            "github_repository_access_denied"
        );
        assert!(
            github.reads.load(std::sync::atomic::Ordering::SeqCst) > before_reads,
            "Git delivery must still make fresh provider checks"
        );
        application.close().await;
        pool.close().await;
        sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
            .execute(&admin)
            .await
            .unwrap();
        sqlx::query("DROP OWNED BY zuse_cloud_staging")
            .execute(&admin)
            .await
            .unwrap();
        sqlx::query("DROP ROLE zuse_cloud_staging")
            .execute(&admin)
            .await
            .unwrap();
        return;
    }
    let op = row["request_config"]["rustAllocationOperationId"]
        .as_str()
        .unwrap();
    let credential = "test-preparation-credential";
    sqlx::query("UPDATE api_cloud_workspaces SET runtime_credential_hash=$2,request_config=request_config||$3 WHERE workspace_id=$1")
        .bind(id).bind(format!("{:x}",Sha256::digest(credential.as_bytes())))
        .bind(json!({"runtimeCredentialExpiresAtMs":now+60000,"runtimeBootstrapReceipt":{"workspaceId":id,"generation":1,"gatewayEpoch":1}})).execute(&pool).await.unwrap();
    let mut update = zuse_protocol::cloud_computers::RuntimePreparationUpdate {
        operation_id: op.into(),
        stage: zuse_protocol::cloud_computers::PreparationStage::Repository,
        state: zuse_protocol::cloud_computers::PreparationStepState::Running,
        source_commit: None,
        error_code: None,
        log: None,
    };
    assert_eq!(
        control
            .preparation_update(id, "wrong-credential", update.clone(), now)
            .await
            .unwrap_err()
            .code,
        "workspace_runtime_fenced"
    );
    assert_eq!(
        control
            .preparation_update(id, credential, update.clone(), now)
            .await
            .unwrap()["applied"],
        true
    );
    update.state = zuse_protocol::cloud_computers::PreparationStepState::Completed;
    update.source_commit = Some("d".repeat(40));
    control
        .preparation_update(id, credential, update.clone(), now + 1)
        .await
        .unwrap();
    assert_eq!(
        control
            .preparation_update(id, credential, update.clone(), now + 2)
            .await
            .unwrap()["applied"],
        false
    );
    let public = store.workspace(&access, id).await.unwrap().public();
    assert_eq!(public["resolvedSourceCommit"], "d".repeat(40));
    assert_eq!(public["preparation"]["repository"]["state"], "completed");
    assert_ne!(
        public["state"], "ready",
        "Preparation facts must not admit the agent"
    );
    assert!(!public.to_string().contains(credential));
    update.operation_id = "another-allocation".into();
    assert_eq!(
        control
            .preparation_update(id, credential, update, now + 3)
            .await
            .unwrap_err()
            .code,
        "runtime_preparation_operation_fenced"
    );
    let (_, is_new) = control
        .create_workspace(&access, copy(&body), now)
        .await
        .unwrap();
    assert!(!is_new);
    let reference = format!("{session}/tool-part");
    sqlx::query(
        "INSERT INTO zuse_rust_tool_output_refs(workspace_id,blob_ref,session_id) VALUES($1,$2,$3)",
    )
    .bind(id)
    .bind(&reference)
    .bind(session)
    .execute(&pool)
    .await
    .unwrap();
    let chunk = zuse_cloud_runtime::tool_output::ToolOutputChunk {
        blob_ref: reference.clone(),
        offset: 0,
        next_offset: 19,
        total_bytes: 19,
        text: "Exact cached output".into(),
    };
    assert_eq!(
        control
            .tool_output_upload(id, "wrong-credential", chunk.clone(), now + 4)
            .await
            .unwrap_err()
            .code,
        "workspace_runtime_fenced"
    );
    control
        .tool_output_upload(id, credential, chunk.clone(), now + 4)
        .await
        .unwrap();
    control
        .tool_output_upload(id, credential, chunk.clone(), now + 5)
        .await
        .unwrap();
    let output = control
        .tool_output_read(&access, id, &reference, 0, 65536)
        .await
        .unwrap();
    assert_eq!(output["text"], "Exact cached output");
    assert_eq!(output["done"], true);
    assert_eq!(
        control
            .tool_output_read(&stranger, id, &reference, 0, 65536)
            .await
            .unwrap_err()
            .code,
        "cloud_workspace_not_found"
    );
    let mut changed = chunk;
    changed.text = "Other cached output".into();
    assert_eq!(
        control
            .tool_output_upload(id, credential, changed, now + 6)
            .await
            .unwrap_err()
            .code,
        "tool_output_immutable_conflict"
    );
    let cipher: String = sqlx::query_scalar(
        "SELECT ciphertext FROM zuse_rust_tool_output_chunks WHERE workspace_id=$1 AND blob_ref=$2",
    )
    .bind(id)
    .bind(&reference)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(!cipher.contains("Exact cached output"));
    // An existing account image cannot prove the selected-connection/harness contract.
    sqlx::query("INSERT INTO api_cloud_project_builds(build_id,project_id,account_id,provider,template_version,configuration_digest,state,snapshot_id,settings,idempotency_key,next_action_at,created_at,updated_at) VALUES('legacy-image','project','user-test','boxd','legacy','fixture','ready','legacy-snapshot','{\"runtimeRpcProtocol\":\"zuse-rpc-v1\",\"rustRuntimeStorageFormat\":\"zuse-rust-engine-v1\",\"rustSessionCommandVersion\":1,\"rustRuntimeCheckoutProjectId\":\"project\"}','legacy-key',0,$1,$1)").bind(now).execute(&pool).await.unwrap();
    sqlx::query("UPDATE api_cloud_projects SET state='ready'")
        .execute(&pool)
        .await
        .unwrap();
    // Retiring affects future admission, never identity or receipts of admitted work.
    sqlx::query("UPDATE cloud_managed_bases SET state='retired'")
        .execute(&pool)
        .await
        .unwrap();
    let mut restore_tx = application.begin().await.unwrap();
    assert_eq!(
        retained_source(&mut restore_tx, "boxd", &r.release_id)
            .await
            .unwrap(),
        r.artifact_id
    );
    restore_tx.commit().await.unwrap();
    let c = candidate::ManagedBaseCandidate {
        provider: r.provider,
        release_id: format!("{}-candidate", r.release_id),
        artifact_id: r.artifact_id.clone(),
        provider_artifact_binding: None,
        initial_runtime_bootstrap: None,
        architecture: r.architecture.clone(),
        capabilities: r.capabilities.clone(),
        manifest: r.manifest.clone(),
        harnesses: r.harnesses.clone(),
        harness_release: "d".repeat(64),
        runtime_sha256: "e".repeat(64),
        provenance_report_sha256: "f".repeat(64),
    };
    let pin = Arc::new(c.pin("user-test", now + 60000, true).unwrap());
    c.stage(&pool, &pin, now).await.unwrap();
    let candidate_control = RuntimeControl::from_pool(
        application.clone(),
        MasterKey::parse(&key).unwrap(),
        TicketSigner::from_jwk(&jwk.to_string(), "https://api.example.test").unwrap(),
    )
    .with_creation_accounts(vec!["user-test".into()])
    .with_credential_authority(control.credential_authority().unwrap())
    .with_staging_base_pin(Some(pin.clone()));
    candidate_control
        .configure_scope_authority(identity.clone(), Some(github.github.clone()), vec![])
        .unwrap();
    // No live entitlement or billing capacity: the exact bounded candidate pin
    // must authorize both admission AND every worker pre-effect checkpoint.
    sqlx::query("UPDATE api_entitlements SET status='ended',paid_through=$1")
        .bind(now - 1)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE api_cloud_billing_periods SET status='closed'")
        .execute(&pool)
        .await
        .unwrap();
    let mut candidate_body = copy(&body);
    candidate_body.idempotency_key = "candidate-task-key".into();
    candidate_body.branch = Some("task/candidate".into());
    let (candidate_created, candidate_new) = candidate_control
        .create_workspace(&access, candidate_body, now)
        .await
        .unwrap();
    assert!(candidate_new);
    let selected:Option<String>=sqlx::query_scalar("SELECT request_config->>'managedCandidateSha256' FROM api_cloud_workspaces WHERE workspace_id=$1").bind(candidate_created["workspace"]["workspaceId"].as_str()).fetch_one(&application).await.unwrap();
    assert_eq!(selected.as_deref(), Some(pin.candidate_sha256.as_str()));
    let candidate_id = candidate_created["workspace"]["workspaceId"]
        .as_str()
        .unwrap();
    if expanded_capacity_only {
        let mut second_candidate_body = copy(&body);
        second_candidate_body.idempotency_key = "second-candidate-limit".into();
        second_candidate_body.branch = Some("task/second-candidate-limit".into());
        let (_, second_new) = candidate_control
            .create_workspace(&access, second_candidate_body, now)
            .await
            .unwrap();
        assert!(
            second_new,
            "one retained pilot computer must not block a second"
        );
        // The staging owner can explicitly raise the bounded pilot allowance
        // without deleting retained work or changing normal paid admission.
        let mut third = copy(&body);
        third.idempotency_key = "third-candidate-explicit-allowance".into();
        third.branch = Some("task/third-candidate".into());
        assert_eq!(
            candidate_control
                .create_workspace(&access, copy(&third), now)
                .await
                .unwrap_err()
                .code,
            "staging_candidate_computer_limit"
        );
        let mut expanded_pin = (*pin).clone();
        expanded_pin.max_computers = Some(3);
        let expanded_control = RuntimeControl::from_pool(
            application.clone(),
            MasterKey::parse(&key).unwrap(),
            TicketSigner::from_jwk(&jwk.to_string(), "https://api.example.test").unwrap(),
        )
        .with_creation_accounts(vec!["user-test".into()])
        .with_credential_authority(control.credential_authority().unwrap())
        .with_staging_base_pin(Some(Arc::new(expanded_pin.clone())));
        expanded_control
            .configure_scope_authority(identity, Some(github.github.clone()), vec![])
            .unwrap();
        let (third_created, third_new) = expanded_control
            .create_workspace(&access, copy(&third), now)
            .await
            .unwrap();
        assert!(third_new);
        let (replayed, replay_new) = expanded_control
            .create_workspace(&access, third, now)
            .await
            .unwrap();
        assert!(!replay_new);
        assert_eq!(
            third_created["workspace"]["workspaceId"],
            replayed["workspace"]["workspaceId"]
        );
        let third_row: Value = sqlx::query_scalar(
            "SELECT to_jsonb(w) FROM api_cloud_workspaces w WHERE workspace_id=$1",
        )
        .bind(third_created["workspace"]["workspaceId"].as_str())
        .fetch_one(&pool)
        .await
        .unwrap();
        let mut tx = application.begin().await.unwrap();
        assert!(
            candidate::compute_allowance(&mut tx, Some(&expanded_pin), &third_row, now)
                .await
                .unwrap()
        );
        tx.rollback().await.unwrap();
        let mut fourth = copy(&body);
        fourth.idempotency_key = "fourth-candidate-over-limit".into();
        fourth.branch = Some("task/fourth-candidate".into());
        assert_eq!(
            expanded_control
                .create_workspace(&access, fourth, now)
                .await
                .unwrap_err()
                .code,
            "staging_candidate_computer_limit"
        );
        let mut unlimited_pin = expanded_pin.clone();
        unlimited_pin.max_computers = None;
        unlimited_pin.unlimited_computers = true;
        let unlimited_control = RuntimeControl::from_pool(
            application.clone(),
            MasterKey::parse(&key).unwrap(),
            TicketSigner::from_jwk(&jwk.to_string(), "https://api.example.test").unwrap(),
        )
        .with_creation_accounts(vec!["user-test".into()])
        .with_credential_authority(control.credential_authority().unwrap())
        .with_staging_base_pin(Some(Arc::new(unlimited_pin.clone())));
        unlimited_control
            .configure_scope_authority(
                Arc::new(crate::github::native::admission_fixture::PersonalIdentity),
                Some(github.github.clone()),
                vec![],
            )
            .unwrap();
        for index in 0..12 {
            let mut next = copy(&body);
            next.idempotency_key = format!("uncapped-{index}");
            next.branch = Some(format!("task/uncapped-{index}"));
            let (created, new) = unlimited_control
                .create_workspace(&access, next, now)
                .await
                .unwrap();
            assert!(new);
            let row: Value = sqlx::query_scalar(
                "SELECT to_jsonb(w) FROM api_cloud_workspaces w WHERE workspace_id=$1",
            )
            .bind(created["workspace"]["workspaceId"].as_str())
            .fetch_one(&pool)
            .await
            .unwrap();
            let mut tx = application.begin().await.unwrap();
            assert!(
                candidate::compute_allowance(&mut tx, Some(&unlimited_pin), &row, now)
                    .await
                    .unwrap()
            );
            tx.rollback().await.unwrap();
        }
        application.close().await;
        pool.close().await;
        sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
            .execute(&admin)
            .await
            .unwrap();
        sqlx::query("DROP OWNED BY zuse_cloud_staging")
            .execute(&admin)
            .await
            .unwrap();
        sqlx::query("DROP ROLE zuse_cloud_staging")
            .execute(&admin)
            .await
            .unwrap();
        return;
    }
    sqlx::query("UPDATE zuse_rust_jobs SET state='completed',lease_token=NULL,lease_expires_at=NULL WHERE kind='workspace-allocate' AND resource_id<>$1").bind(candidate_id).execute(&pool).await.unwrap();
    let provider = AllowanceProvider {
        pool: pool.clone(),
        release: c.release_id.clone(),
        mode: 0,
        machine: Default::default(),
        creates: Default::default(),
        launches: Default::default(),
        launch_receipt: Default::default(),
    };
    candidate_control
        .allocate_once(&provider, chrono::Utc::now().timestamp_millis() + 10000)
        .await
        .unwrap();
    candidate_control
        .allocate_once(&provider, chrono::Utc::now().timestamp_millis() + 10000)
        .await
        .unwrap();
    assert_eq!(
        provider.creates.load(std::sync::atomic::Ordering::SeqCst),
        1
    );
    assert_eq!(
        provider.launches.load(std::sync::atomic::Ordering::SeqCst),
        1
    );
    let candidate_row: Value =
        sqlx::query_scalar("SELECT to_jsonb(w) FROM api_cloud_workspaces w WHERE workspace_id=$1")
            .bind(candidate_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    for bad in [
        {
            let mut p = (*pin).clone();
            p.expires_at = now - 1;
            p
        },
        {
            let mut p = (*pin).clone();
            p.manifest_sha256 = "0".repeat(64);
            p
        },
    ] {
        let mut tx = application.begin().await.unwrap();
        assert!(
            candidate::compute_allowance(
                &mut tx,
                Some(&bad),
                &candidate_row,
                chrono::Utc::now().timestamp_millis()
            )
            .await
            .is_err()
        );
        tx.rollback().await.unwrap();
    }
    let mut second_candidate_body = copy(&body);
    second_candidate_body.idempotency_key = "second-candidate-limit".into();
    second_candidate_body.branch = Some("task/second-candidate-limit".into());
    let (_, second_new) = candidate_control
        .create_workspace(&access, second_candidate_body, now)
        .await
        .unwrap();
    assert!(
        second_new,
        "one retained pilot computer must not block a second"
    );
    let mut tx = application.begin().await.unwrap();
    sqlx::query("UPDATE api_cloud_workspaces SET request_config=jsonb_set(request_config,'{managedCandidateSha256}','\"extra\"') WHERE workspace_id=$1").bind(id).execute(&mut *tx).await.unwrap();
    assert_eq!(
        candidate::compute_allowance(&mut tx, Some(&pin), &candidate_row, now)
            .await
            .unwrap_err()
            .code,
        "staging_candidate_computer_limit"
    );
    tx.rollback().await.unwrap();
    // Revoke while create is in flight. Known and lost-reply outcomes must bind
    // the ORIGINAL resource; no second create or runtime launch is permitted.
    for mode in [1u8, 2u8] {
        sqlx::query("UPDATE api_cloud_workspaces SET state='deleted',desired_state='deleted' WHERE request_config ? 'managedCandidateSha256'").execute(&pool).await.unwrap();
        sqlx::query("UPDATE zuse_rust_jobs SET state='completed',lease_token=NULL,lease_expires_at=NULL WHERE kind='workspace-allocate'").execute(&pool).await.unwrap();
        sqlx::query("UPDATE cloud_managed_bases SET state='preparing' WHERE release_id=$1")
            .bind(&c.release_id)
            .execute(&pool)
            .await
            .unwrap();
        let mut next = copy(&body);
        next.idempotency_key = format!("revocation-{mode}");
        next.branch = Some(format!("task/revocation-{mode}"));
        let (created, _) = candidate_control
            .create_workspace(&access, next, chrono::Utc::now().timestamp_millis())
            .await
            .unwrap();
        let workspace = created["workspace"]["workspaceId"].as_str().unwrap();
        let p = AllowanceProvider {
            pool: pool.clone(),
            release: c.release_id.clone(),
            mode,
            machine: Default::default(),
            creates: Default::default(),
            launches: Default::default(),
            launch_receipt: Default::default(),
        };
        let first = candidate_control
            .allocate_once(&p, chrono::Utc::now().timestamp_millis() + 10000)
            .await;
        // A positive create immediately continues to the next fresh fence;
        // retirement during create must now reject this same worker pass.
        if mode == 1 {
            assert_eq!(
                first.err().unwrap().code,
                "staging_managed_base_candidate_unavailable"
            );
        } else {
            assert!(first.is_err());
        }
        assert!(
            candidate_control
                .allocate_once(&p, chrono::Utc::now().timestamp_millis() + 10000)
                .await
                .is_err()
        );
        let saved: Option<String> = sqlx::query_scalar(
            "SELECT machine_id FROM zuse_rust_allocations WHERE workspace_id=$1",
        )
        .bind(workspace)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(saved, Some(format!("allowance-resource-{mode}")));
        assert_eq!(p.creates.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(p.launches.load(std::sync::atomic::Ordering::SeqCst), 0);
    }
    sqlx::query("UPDATE api_entitlements SET status='active',paid_through=$1")
        .bind(now + 86400000)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE api_cloud_billing_periods SET status='manual'")
        .execute(&pool)
        .await
        .unwrap();

    assert!(
        !control
            .create_workspace(&access, copy(&body), now)
            .await
            .unwrap()
            .1
    );
    let mut next = copy(&body);
    next.idempotency_key = "next-key".into();
    next.branch = Some("task/next".into());
    assert_eq!(
        control
            .create_workspace(&access, copy(&next), now)
            .await
            .unwrap_err()
            .code,
        "managed_base_unavailable_for_agent"
    );
    sqlx::query("UPDATE cloud_managed_bases SET state='verified'")
        .execute(&pool)
        .await
        .unwrap();
    // A private repository still requires the managed credential helper capability.
    sqlx::query("UPDATE cloud_managed_bases SET runtime_manifest=jsonb_set(runtime_manifest,'{manifest}',(runtime_manifest->'manifest')-'rustGithubCredentialsVersion')")
        .execute(&pool).await.unwrap();
    sqlx::query("UPDATE api_cloud_projects SET visibility='private'")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        control
            .create_workspace(&access, copy(&next), now)
            .await
            .unwrap_err()
            .code,
        "github_runtime_credentials_required"
    );
    sqlx::query("UPDATE cloud_agent_connections SET state='revoked'")
        .execute(&pool)
        .await
        .unwrap();
    let connections = store.agent_connections("user-test").await.unwrap();
    assert!(
        store
            .managed_launch_catalog(&connections)
            .await
            .unwrap()
            .providers
            .is_empty()
    );
    assert_eq!(
        control
            .create_workspace(&access, next, now)
            .await
            .unwrap_err()
            .code,
        "agent_connection_required"
    );
    application.close().await;
    pool.close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .unwrap();
    sqlx::query("DROP OWNED BY zuse_cloud_staging")
        .execute(&admin)
        .await
        .unwrap();
    sqlx::query("DROP ROLE zuse_cloud_staging")
        .execute(&admin)
        .await
        .unwrap();
}
