//! A fresh test process carries operator config, avoiding process-global env
//! mutation while HTTP/runtime threads execute the real durable worker.
use super::*;
use zuse_protocol::cloud_computers::EnvironmentRecipe;
#[tokio::test]
#[ignore = "requires explicit isolated localhost PostgreSQL"]
async fn staging_pilot_is_separate_bounded_and_reconciles_original_effects() {
    let url = std::env::var("ZUSE_CLOUD_TEST_DATABASE_URL").unwrap();
    assert!(matches!(
        reqwest::Url::parse(&url).unwrap().host_str(),
        Some("localhost" | "127.0.0.1" | "::1")
    ));
    if let Ok(schema) = std::env::var("ZUSE_CACHE_PILOT_TEST_CHILD") {
        assert!(
            schema
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_')
        );
        let pool = pool(&url, &schema).await;
        super::boxd::exercise_staging_pilot(&pool).await;
        pool.close().await;
        return;
    }
    let admin = PgPool::connect(&url).await.unwrap();
    let schema = format!("pilot_{}", uuid::Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&admin)
        .await
        .unwrap();
    let pool = pool(&url, &schema).await;
    for script in [
        include_str!("../../tests/fixtures/legacy-catalog.sql"),
        include_str!("../../tests/fixtures/projects.sql"),
        include_str!("../../migrations/0009_computers.sql"),
        include_str!("../../migrations/0023_managed_base_admission_lock.sql"),
        include_str!("../../migrations/0020_staging_managed_base_candidates.sql"),
        MIGRATION,
    ] {
        sqlx::raw_sql(script).execute(&pool).await.unwrap();
    }
    sqlx::raw_sql("ALTER TABLE cloud_managed_bases ADD COLUMN runtime_manifest jsonb; DELETE FROM api_cloud_workspaces WHERE workspace_id<>'ws-alice'; UPDATE api_cloud_workspaces SET request_config='{\"runtimeGeneration\":1}' WHERE workspace_id='ws-alice'; INSERT INTO api_cloud_projects(project_id,account_id,repository_identity,repository_url,display_name,default_branch,visibility,git_connection_kind,configuration_digest,state,idempotency_key,created_at,updated_at) VALUES('project','owner','github.com/example/project','https://github.com/example/project.git','fixture','main','public','github-app','fixture','connected','fixture',0,0);").execute(&pool).await.unwrap();
    let now = chrono::Utc::now().timestamp_millis();
    let candidate = crate::managed_bases::candidate::fixture_project_cache_candidate();
    let pin = candidate.pin("owner", now + 3_600_000, true).unwrap();
    candidate.stage(&pool, &pin, now).await.unwrap();
    let good = EnvironmentRecipe::default();
    let mut failing = good.clone();
    failing.scripts.initialize = Some("exit 23".into());
    use sha2::Digest;
    let inputs = [&good, &failing]
        .into_iter()
        .map(|recipe| pilot::Input {
            recipe_digest: format!(
                "{:x}",
                sha2::Sha256::digest(serde_json::to_vec(recipe).unwrap())
            ),
            recipe_source: "project".into(),
            source_commit: "c".repeat(40),
            dependency_digest: format!(
                "{:x}",
                sha2::Sha256::digest(
                    serde_json::to_vec(&(
                        "c".repeat(40),
                        std::collections::BTreeMap::<String, String>::new()
                    ))
                    .unwrap()
                )
            ),
        })
        .collect();
    let pilot = pilot::Pilot {
        version: 1,
        pilot_id: "isolated-case".into(),
        owner_id: "owner".into(),
        actor_id: "owner".into(),
        project_id: "project".into(),
        repository_identity: "github.com/example/project".into(),
        candidate_sha256: pin.candidate_sha256.clone(),
        base_release_id: candidate.release_id.clone(),
        base_artifact_id: candidate.artifact_id.clone(),
        base_artifact_version: "1".into(),
        runtime_sha256: candidate.runtime_sha256.clone(),
        architecture: candidate.architecture.clone(),
        expires_at: now + 1_200_000,
        protected_workspace_id: "ws-alice".into(),
        protected_machine_id: "machine-alice".into(),
        max_active_machines: 2,
        max_owned_machines: 1,
        inputs,
        mechanism_capability: CacheCapabilityReceipt {
            version: 1,
            runtime_sha256: candidate.runtime_sha256.clone(),
            environment_cache_version: 1,
            secret_free_source_bundle_version: 1,
            retained_task_storage_absent_required: true,
            credential_free_preparation_required: true,
            preparation_capture_version: 1,
        },
    };
    pilot.validate(&pin, "staging", now).unwrap();
    assert!(pilot.validate(&pin, "production", now).is_err());
    assert!(pilot.validate(&pin, "staging", pilot.expires_at).is_err());
    let mut wrong = pilot.clone();
    wrong.actor_id = "another-actor".into();
    assert!(wrong.validate(&pin, "staging", now).is_err());
    let mut wrong = pilot.clone();
    wrong.max_owned_machines = 2;
    assert!(wrong.validate(&pin, "staging", now).is_err());
    let child=std::process::Command::new(std::env::current_exe().unwrap())
        .args(["project_caches::pilot_tests::staging_pilot_is_separate_bounded_and_reconciles_original_effects","--exact","--ignored","--nocapture","--test-threads=1"])
        .env("ZUSE_CACHE_PILOT_TEST_CHILD",&schema).env("ZUSE_CLOUD_CREATE_ACCOUNTS","owner")
        .env("ZUSE_CLOUD_AUTH_WRITE_ACCOUNTS","owner").env("ZUSE_DEPLOYMENT_ENVIRONMENT","staging")
        .env("ZUSE_CLOUD_STAGING_BASE_PIN",serde_json::to_string(&pin).unwrap())
        .env("ZUSE_CLOUD_STAGING_PROJECT_CACHE_PILOT",serde_json::to_string(&pilot).unwrap())
        .env("ZUSE_CLOUD_PROJECT_PREPARATION_ENABLED","1").env("ZUSE_CLOUD_PROJECT_PREPARATION_BACKGROUND","0")
        .output().unwrap();
    let stdout = String::from_utf8_lossy(&child.stdout);
    let stderr = String::from_utf8_lossy(&child.stderr);
    pool.close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .unwrap();
    admin.close().await;
    assert!(
        child.status.success(),
        "isolated pilot child failed:\n{stdout}\n{stderr}"
    );
}
async fn pool(url: &str, schema: &str) -> PgPool {
    let schema = schema.to_owned();
    sqlx::postgres::PgPoolOptions::new()
        .max_connections(4)
        .after_connect(move |c, _| {
            let schema = schema.clone();
            Box::pin(async move {
                sqlx::query(&format!("SET search_path={schema}"))
                    .execute(c)
                    .await?;
                Ok(())
            })
        })
        .connect(url)
        .await
        .unwrap()
}
