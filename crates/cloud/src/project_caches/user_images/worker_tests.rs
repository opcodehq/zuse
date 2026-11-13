//! The durable worker against PostgreSQL and the Boxd phases over the
//! in-memory provider: save → debounce → build → ready; failure keeps the
//! ready image; lost replies reconcile; a newer recipe supersedes a waiting one.
use super::boxd::BoxdUserImages;
use super::tests::FakeHost;
use super::worker::*;
use super::*;
use serde_json::json;
use sqlx::PgPool;
use std::{collections::BTreeMap, sync::Arc};
use zuse_protocol::cloud_computers::SaveMachine;

pub(crate) fn release_manifest(release: &str, artifact: &str) -> Value {
    json!({
        "provider":"boxd","releaseId":release,"artifactId":artifact,"architecture":"x86_64",
        "certificationReportSha256":"a".repeat(64),
        "initialRuntimeBootstrap":{"version":1,"mode":"stopped-memory-snapshot-v1","runtimeSha256":"a".repeat(64),"supervisorSha256":"b".repeat(64),"serviceSha256":"c".repeat(64),"manifestSha256":"d".repeat(64),"helperSha256":"e".repeat(64)},
        "capabilities":{"id":"boxd","configured":true,"managedBaseReady":true,"preservesDisk":true,"preservesMemory":true,"supportsSnapshots":true,"supportsDocker":true,"unavailableReason":null,"sizes":[{"id":"default","label":"Standard","vcpus":2,"memoryMib":2048,"diskGib":20}]},
        "harnesses":[{"agent":"codex","version":"fixture-1","sha256":"b".repeat(64),"catalog":{"id":"codex","name":"Codex","defaultModel":"fixture","models":[{"id":"fixture","label":"Fixture"}]}}],
        "manifest":{"managedBase":true,"rustRuntimeSupervisorVersion":1,"version":1,"buildId":release,"templateVersion":"fixture","configurationDigest":"fixture","planDigest":"c".repeat(64),"runtimeRpcProtocol":"zuse-rpc-v1","rustRuntimeStorageFormat":zuse_protocol::cloud::RUST_RUNTIME_STORAGE_FORMAT,"rustSessionCommandVersion":1,"credentialsRemoved":true,"repositories":[],"rustRepositoryFetchVersion":1,"rustEnvironmentRecipeVersion":1,"rustLaunchModelConfigVersion":1,"rustLaunchAssetsVersion":1}
    })
}

pub(crate) struct Database {
    pub admin: PgPool,
    pub pool: PgPool,
    pub schema: String,
}
impl Database {
    pub async fn new(prefix: &str) -> Self {
        let url =
            std::env::var("ZUSE_CLOUD_TEST_DATABASE_URL").expect("isolated PostgreSQL required");
        let parsed = reqwest::Url::parse(&url).unwrap();
        assert!(matches!(
            parsed.host_str(),
            Some("127.0.0.1" | "localhost" | "::1")
        ));
        let admin = PgPool::connect(&url).await.unwrap();
        let schema = format!("{prefix}_{}", uuid::Uuid::new_v4().simple());
        sqlx::query(&format!("CREATE SCHEMA {schema}"))
            .execute(&admin)
            .await
            .unwrap();
        let name = schema.clone();
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(6)
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
            include_str!("../../../tests/fixtures/legacy-catalog.sql"),
            include_str!("../../../tests/fixtures/projects.sql"),
            include_str!("../../../migrations/0009_computers.sql"),
            "ALTER TABLE cloud_managed_bases ADD COLUMN runtime_manifest jsonb",
            include_str!("../../../migrations/0023_managed_base_admission_lock.sql"),
            include_str!("../../../migrations/0031_machine_images.sql"),
            MIGRATION,
            // Applying it twice is a no-op.
            MIGRATION,
            "CREATE TABLE api_cloud_billing_periods(period_id text PRIMARY KEY,account_id text,status text,currency text,period_start bigint,period_end bigint,included_provider_cost_micros bigint,markup_basis_points bigint,overage_cap_micros bigint); CREATE TABLE api_cloud_billing_ledger(period_id text,kind text,amount_micros bigint); CREATE TABLE api_cloud_billing_reservations(period_id text,provider_cost_micros bigint,expires_at bigint); INSERT INTO api_cloud_billing_periods VALUES('period','user-alice','manual','USD',0,9999999999999,0,0,0);",
        ] {
            sqlx::raw_sql(script).execute(&pool).await.unwrap();
        }
        Self {
            admin,
            pool,
            schema,
        }
    }
    pub async fn base(&self, release: &str, artifact: &str, created: i64) {
        sqlx::query("INSERT INTO cloud_managed_bases(provider,release_id,artifact_id,architecture,capabilities,state,created_at,runtime_manifest) VALUES('boxd',$1,$2,'x86_64',$3,'verified',$4,$5)")
            .bind(release).bind(artifact).bind(release_manifest(release, artifact)["capabilities"].clone()).bind(created).bind(release_manifest(release, artifact))
            .execute(&self.pool).await.unwrap();
    }
    pub async fn drop(self) {
        self.pool.close().await;
        sqlx::query(&format!("DROP SCHEMA {} CASCADE", self.schema))
            .execute(&self.admin)
            .await
            .unwrap();
        self.admin.close().await;
    }
}

pub(crate) async fn save(pool: &PgPool, owner: &str, expected: i64, script: &str, now: i64) {
    crate::computers::ComputerControl::from_pool(pool.clone(), vec![owner.into()])
        .save_machine_as(
            owner,
            owner,
            None,
            SaveMachine {
                expected_revision: expected,
                recipe: MachineRecipe {
                    setup_script: script.into(),
                    ..Default::default()
                },
            },
            now,
        )
        .await
        .unwrap();
}

pub(crate) fn control(pool: &PgPool, host: &FakeHost) -> UserImageControl {
    let control = UserImageControl::from_pool(pool.clone());
    let mut backends: BTreeMap<ProviderId, Arc<dyn UserImageBackend>> = BTreeMap::new();
    backends.insert(
        ProviderId::Boxd,
        Arc::new(BoxdUserImages::new(Arc::new(host.clone()))),
    );
    control.install(backends).unwrap();
    control
}

pub(crate) async fn run(control: &UserImageControl, mut now: i64, rounds: usize) -> i64 {
    for _ in 0..rounds {
        // Each tick may defer by a few seconds; time moves on like the worker loop.
        control.work_once(now).await.unwrap();
        now += 11_000;
    }
    now
}

async fn images(pool: &PgPool) -> Vec<Value> {
    sqlx::query_scalar(
        "SELECT to_jsonb(i) FROM cloud_machine_images i ORDER BY created_at,image_id",
    )
    .fetch_all(pool)
    .await
    .unwrap()
}

#[tokio::test]
#[ignore = "requires explicit isolated localhost PostgreSQL"]
async fn saved_setup_script_builds_one_verified_image_and_failures_keep_it() {
    let db = Database::new("uimg").await;
    let pool = db.pool.clone();
    db.base("base-v1", "base-snapshot", 1).await;
    let host = FakeHost::new();
    let control = control(&pool, &host);
    // Instructions-only machine: nothing to build.
    crate::computers::ComputerControl::from_pool(pool.clone(), vec!["user-alice".into()])
        .save_machine_as(
            "user-alice",
            "user-alice",
            None,
            SaveMachine {
                expected_revision: 0,
                recipe: MachineRecipe {
                    instructions: "Prefer pnpm".into(),
                    ..Default::default()
                },
            },
            1_000,
        )
        .await
        .unwrap();
    assert!(images(&pool).await.is_empty());
    // Two quick saves queue one build of the newest script, after the debounce.
    save(&pool, "user-alice", 1, "npm i -g pnpm", 2_000).await;
    save(&pool, "user-alice", 2, "npm i -g pnpm yarn", 3_000).await;
    let queued = images(&pool).await;
    assert_eq!(queued.len(), 1);
    assert_eq!(queued[0]["state"], "queued");
    assert_eq!(queued[0]["recipe_revision"], 3);
    assert_eq!(queued[0]["next_attempt_at"], 3_000 + SAVE_DEBOUNCE_MS);
    assert!(!control.work_once(4_000).await.unwrap(), "debounced");
    let now = run(&control, 3_000 + SAVE_DEBOUNCE_MS, 40).await;
    let built = images(&pool).await;
    assert_eq!(built[0]["state"], "ready", "{built:?}");
    assert_eq!(built[0]["base_release_id"], "base-v1");
    assert_eq!(built[0]["base_artifact_version"], "3");
    assert!(
        built[0]["artifact_id"]
            .as_str()
            .unwrap()
            .starts_with("snap-")
    );
    assert!(built[0]["built_at"].as_i64().unwrap() > 0);
    assert_eq!(built[0]["plan"]["setupScript"], "npm i -g pnpm yarn");
    {
        let state = host.0.lock().unwrap();
        assert_eq!(
            (state.creates, state.setup_launches, state.captures),
            (2, 1, 1)
        );
        assert!(state.machines.is_empty());
    }
    let ready_artifact = built[0]["artifact_id"].clone();

    // A failing script: the build fails with its log, cleanup removes its
    // machines and snapshot, and the earlier ready image stays ready.
    {
        let mut state = host.0.lock().unwrap();
        state.guest.setup_exit = 1;
        state.guest.setup_log =
            "E: Unable to locate package nope\nTOKEN=ghp_abcdefghijklmnopqrstuvwxyz0123\n".into();
        state.setup_launches = 0;
        state.setup_polls = 0;
    }
    save(&pool, "user-alice", 3, "apt-get install -y nope", now).await;
    let now = run(&control, now + SAVE_DEBOUNCE_MS, 30).await;
    let all = images(&pool).await;
    assert_eq!(all.len(), 2);
    assert_eq!(all[0]["state"], "ready");
    assert_eq!(all[0]["artifact_id"], ready_artifact);
    assert_eq!(all[1]["state"], "failed", "{all:?}");
    assert_eq!(all[1]["error_code"], "user_image_setup_failed");
    let log = all[1]["log_excerpt"].as_str().unwrap();
    assert!(log.contains("Unable to locate package nope"));
    assert!(!log.contains("ghp_"));
    {
        let state = host.0.lock().unwrap();
        assert!(state.machines.is_empty());
        assert_eq!(state.snapshots.len(), 2, "base and the ready image only");
    }

    // Re-saving the identical script as the ready image is superseded, not rebuilt.
    host.0.lock().unwrap().guest.setup_exit = 0;
    save(&pool, "user-alice", 4, "npm i -g pnpm yarn", now).await;
    run(&control, now + SAVE_DEBOUNCE_MS, 4).await;
    let all = images(&pool).await;
    assert_eq!(all[2]["state"], "superseded");
    assert_eq!(all[2]["identity_key"], all[0]["identity_key"]);
    db.drop().await;
}

#[tokio::test]
#[ignore = "requires explicit isolated localhost PostgreSQL"]
async fn builds_wait_for_capacity_and_a_base_and_reconcile_lost_replies() {
    let db = Database::new("uimgw").await;
    let pool = db.pool.clone();
    let host = FakeHost::new();
    let control = control(&pool, &host);
    save(&pool, "user-alice", 0, "npm i -g pnpm", 1_000).await;
    // No verified base yet: the build waits with a reason.
    run(&control, 1_000 + SAVE_DEBOUNCE_MS, 1).await;
    let row = &images(&pool).await[0];
    assert_eq!(
        (row["state"].as_str(), row["error_code"].as_str()),
        (Some("queued"), Some("user_image_managed_base_unavailable"))
    );
    db.base("base-v1", "base-snapshot", 1).await;
    // No billing capacity: still waiting, no machine created.
    sqlx::query("UPDATE api_cloud_billing_periods SET account_id='someone-else'")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE cloud_machine_images SET next_attempt_at=0")
        .execute(&pool)
        .await
        .unwrap();
    control.work_once(100_000).await.unwrap();
    let row = &images(&pool).await[0];
    assert_eq!(row["state"], "queued");
    assert_eq!(row["error_code"], "cloud_billing_period_missing");
    assert_eq!(host.0.lock().unwrap().creates, 0);
    sqlx::query("UPDATE api_cloud_billing_periods SET account_id='user-alice'")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE cloud_machine_images SET next_attempt_at=0")
        .execute(&pool)
        .await
        .unwrap();
    // Lose the builder create reply and the capture reply: each is found by
    // its build-unique name, never created twice.
    host.0.lock().unwrap().lose_reply = Some("create");
    let now = run(&control, 200_000, 6).await;
    host.0.lock().unwrap().lose_reply = Some("capture");
    run(&control, now, 40).await;
    let row = &images(&pool).await[0];
    assert_eq!(row["state"], "ready", "{row:?}");
    {
        let state = host.0.lock().unwrap();
        assert_eq!(
            (state.creates, state.captures, state.setup_launches),
            (2, 1, 1)
        );
    }
    db.drop().await;
}

#[tokio::test]
#[ignore = "requires explicit isolated localhost PostgreSQL"]
async fn deadline_and_retired_base_clean_up_without_publishing() {
    let db = Database::new("uimgd").await;
    let pool = db.pool.clone();
    db.base("base-v1", "base-snapshot", 1).await;
    let host = FakeHost::new();
    host.0.lock().unwrap().guest.setup_polls_before_exit = usize::MAX;
    let control = control(&pool, &host);
    save(&pool, "user-alice", 0, "sleep infinity", 1_000).await;
    let now = run(&control, 1_000 + SAVE_DEBOUNCE_MS, 6).await;
    assert_eq!(images(&pool).await[0]["phase"], "setup");
    // Past the script limit plus provider overhead: cleanup, then failed.
    run(&control, now + 3 * 3600 * 1000, 10).await;
    let row = &images(&pool).await[0];
    assert_eq!(
        (row["state"].as_str(), row["error_code"].as_str()),
        (Some("failed"), Some("user_image_build_deadline"))
    );
    assert!(host.0.lock().unwrap().machines.is_empty());

    // A base retired while the build ran never gains the image.
    {
        let mut state = host.0.lock().unwrap();
        state.guest.setup_polls_before_exit = 1;
        state.setup_launches = 0;
        state.setup_polls = 0;
    }
    save(&pool, "user-alice", 1, "npm i -g pnpm", now).await;
    let mut now = now + 4 * 3600 * 1000;
    for _ in 0..40 {
        control.work_once(now).await.unwrap();
        now += 11_000;
        let phase: Option<String> =
            sqlx::query_scalar("SELECT phase FROM cloud_machine_images WHERE state='building'")
                .fetch_optional(&pool)
                .await
                .unwrap()
                .flatten();
        if phase.as_deref() == Some("publish") {
            sqlx::query("UPDATE cloud_managed_bases SET state='retired'")
                .execute(&pool)
                .await
                .unwrap();
        }
    }
    let rows = images(&pool).await;
    assert_eq!(rows[1]["state"], "failed", "{rows:?}");
    assert_eq!(rows[1]["error_code"], "user_image_managed_base_retired");
    {
        let state = host.0.lock().unwrap();
        assert!(state.machines.is_empty());
        assert_eq!(
            state.snapshots.len(),
            1,
            "the unpublished snapshot was deleted"
        );
    }

    db.drop().await;
}
