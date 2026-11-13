use super::worker::{ImageProvider, Preparation};
use super::*;
use crate::auth::Principal;
use crate::providers::{
    Machine, MachineState, ProviderError,
    boxd::{Snapshot, SnapshotCapture, SnapshotState},
};
use async_trait::async_trait;
use std::{collections::BTreeMap, sync::Mutex};
#[derive(Default)]
struct State {
    machines: BTreeMap<String, Machine>,
    snapshots: BTreeMap<String, Snapshot>,
    plans: BTreeMap<String, ImagePlan>,
    events: Vec<String>,
    lost_create: bool,
    lost_capture: bool,
    lost_delete: bool,
    unavailable_delete: bool,
    corrupt: bool,
}
#[derive(Default)]
struct Provider(Mutex<State>);
fn manifest(plan: &ImagePlan) -> ImageManifest {
    serde_json::from_value(json!({"version":1,"buildId":plan.build_id,"templateVersion":plan.template_version,"configurationDigest":plan.configuration_digest,"planDigest":format!("{:x}",Sha256::digest(serde_json::to_vec(plan).unwrap())),"runtimeRpcProtocol":"zuse-rpc-v1","rustRuntimeStorageFormat":zuse_protocol::cloud::RUST_RUNTIME_STORAGE_FORMAT,"rustSessionCommandVersion":1,"credentialsRemoved":true,"repositories":plan.repositories.iter().map(|r|json!({"projectId":r.project_id,"repositoryIdentity":r.repository_identity,"defaultBranch":r.default_branch,"path":plan.cache_root.join(format!("{:x}.git",Sha256::digest(r.repository_identity.as_bytes()))),"sourceCommit":"1234567890123456789012345678901234567890"})).collect::<Vec<_>>()})).unwrap()
}
#[async_trait]
impl ImageProvider for Provider {
    async fn find(&self, id: &str) -> Result<Option<Machine>, ProviderError> {
        let s = self.0.lock().unwrap();
        Ok(s.machines
            .get(id)
            .or_else(|| s.machines.values().find(|m| m.name == id))
            .cloned())
    }
    async fn allocate(&self, snapshot: &str, name: &str) -> Result<Machine, ProviderError> {
        let mut s = self.0.lock().unwrap();
        assert!(snapshot == "rust-base" || s.snapshots.values().any(|v| v.id == snapshot));
        let machine = Machine {
            id: format!("vm-{name}"),
            name: name.into(),
            state: MachineState::Running,
            vcpu: Some(2),
            memory_bytes: Some(4 * 1024 * 1024 * 1024),
        };
        assert!(
            s.machines
                .insert(machine.id.clone(), machine.clone())
                .is_none(),
            "duplicate allocation"
        );
        s.events.push(format!("allocate:{name}"));
        if s.lost_create {
            s.lost_create = false;
            return Err(ProviderError::OutcomeUnknown);
        }
        Ok(machine)
    }
    async fn provenance(&self, _: &str, _: &str, version: u64) -> Result<(), ProviderError> {
        assert_eq!(version, 1);
        Ok(())
    }
    async fn preparation(
        &self,
        machine: &str,
        _: &ImagePlan,
    ) -> Result<Preparation, ProviderError> {
        let s = self.0.lock().unwrap();
        Ok(Preparation {
            manifest: s.plans.get(machine).map(manifest),
            ..Default::default()
        })
    }
    async fn launch(
        &self,
        machine: &str,
        plan: &ImagePlan,
        grants: &[github::Grant],
    ) -> Result<(), ProviderError> {
        assert!(grants.is_empty());
        let mut s = self.0.lock().unwrap();
        s.plans.insert(machine.into(), plan.clone());
        s.events.push("prepare".into());
        Ok(())
    }
    async fn verify(&self, _: &str, plan: &ImagePlan) -> Result<ImageManifest, ProviderError> {
        let mut s = self.0.lock().unwrap();
        s.events.push("verify-cache-readback".into());
        let mut value = manifest(plan);
        if s.corrupt {
            value.credentials_removed = false;
        }
        Ok(value)
    }
    async fn snapshot(&self, name: &str) -> Result<Option<Snapshot>, ProviderError> {
        if name == "rust-base" {
            return Ok(Some(Snapshot {
                id: "rust-base".into(),
                name: "base".into(),
                version: 1,
                state: SnapshotState::Ready,
                size_bytes: 100,
                vcpu: 2,
                memory_bytes: 4 * 1024 * 1024 * 1024,
                created_at_ms: 0,
                updated_at_ms: 0,
            }));
        }
        {
            let s = self.0.lock().unwrap();
            Ok(s.snapshots
                .get(name)
                .or_else(|| s.snapshots.values().find(|v| v.id == name))
                .cloned())
        }
    }
    async fn capture(&self, machine: &str, name: &str) -> Result<SnapshotCapture, ProviderError> {
        let mut s = self.0.lock().unwrap();
        assert!(s.plans.contains_key(machine));
        s.events.push("capture".into());
        assert!(
            s.snapshots
                .insert(
                    name.into(),
                    Snapshot {
                        id: format!("snapshot-{name}"),
                        name: name.into(),
                        version: 1,
                        state: SnapshotState::Ready,
                        size_bytes: 100,
                        vcpu: 2,
                        memory_bytes: 4 * 1024 * 1024 * 1024,
                        created_at_ms: now(),
                        updated_at_ms: now()
                    }
                )
                .is_none()
        );
        if s.lost_capture {
            s.lost_capture = false;
            return Err(ProviderError::OutcomeUnknown);
        }
        Ok(SnapshotCapture {
            id: format!("snapshot-{name}"),
            name: name.into(),
            version: 1,
            state: SnapshotState::Ready,
        })
    }
    async fn destroy(&self, id: &str, name: &str) -> Result<(), ProviderError> {
        let mut s = self.0.lock().unwrap();
        if let Some(m) = s.machines.remove(id) {
            assert_eq!(m.name, name);
            s.events.push(format!("destroy:{name}"));
        }
        Ok(())
    }
    async fn delete_snapshot(&self, id: &str) -> Result<(), ProviderError> {
        let mut s = self.0.lock().unwrap();
        if s.unavailable_delete {
            return Err(ProviderError::Unavailable);
        }
        s.snapshots.retain(|_, v| v.id != id);
        s.events.push("delete-failed-snapshot".into());
        if s.lost_delete {
            s.lost_delete = false;
            return Err(ProviderError::OutcomeUnknown);
        }
        Ok(())
    }
}
struct Identity;
#[async_trait]
impl crate::auth::Identity for Identity {
    async fn verify(&self, token: &str) -> Result<Principal, ApiError> {
        let token = if token == "dev-user"
            && std::env::var_os("ZUSE_CLOUD_IMAGES_UI_FIXTURE_FILE").is_some()
        {
            "alice"
        } else {
            token
        };
        Ok(Principal {
            account_id: format!("user-{token}"),
        })
    }
    async fn membership(
        &self,
        account: &str,
        organization: &str,
    ) -> Result<crate::auth::Membership, ApiError> {
        Ok(crate::auth::Membership {
            id: "member".into(),
            user_id: account.into(),
            organization_id: organization.into(),
            status: "active".into(),
            role: crate::auth::MembershipRole {
                slug: "member".into(),
            },
        })
    }
}
async fn http(
    store: &Store,
    method: &str,
    path: &str,
    scope: Option<&str>,
    body: Option<Value>,
) -> (axum::http::StatusCode, Value) {
    use tower::ServiceExt;
    let app = crate::routes::router(
        crate::routes::AppState {
            store: store.clone(),
            identity: Arc::new(Identity),
            organizations_enabled: true,
            organization_ids: Arc::new(vec!["org-team".into()]),
            runtime: None,
            gateway: Arc::new(crate::gateway::Hub::default()),
        },
        vec![],
    );
    let mut req = axum::http::Request::builder()
        .method(method)
        .uri(path)
        .header("authorization", "Bearer alice")
        .header("content-type", "application/json");
    if let Some(scope) = scope {
        req = req.header("x-zuse-workspace", scope);
    }
    let res = app
        .oneshot(
            req.body(axum::body::Body::from(
                body.map(|b| b.to_string()).unwrap_or_default(),
            ))
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.headers()["cache-control"], "no-store");
    let status = res.status();
    let value = serde_json::from_slice(
        &axum::body::to_bytes(res.into_body(), 1_000_000)
            .await
            .unwrap(),
    )
    .unwrap();
    (status, value)
}
fn access() -> Access {
    Access {
        actor: Principal {
            account_id: "user-alice".into(),
        },
        scope: crate::access::Scope::Personal,
        owner_id: "user-alice".into(),
        membership: None,
    }
}
fn config() -> ImageConfig {
    ImageConfig {
        base_snapshot: "rust-base".into(),
        base_version: 1,
        template_version: "rust-v1".into(),
        max_cache_bytes: 4 * 1024 * 1024 * 1024,
        legacy_builds: true,
    }
}
fn request(key: &str) -> ImageBuildRequest {
    ImageBuildRequest {
        mode: ImageBuildMode::Update,
        provider_id: Some("boxd".into()),
        idempotency_key: key.into(),
    }
}
async fn drain(control: &ImageControl, provider: &Provider) {
    for _ in 0..25 {
        sqlx::query("UPDATE zuse_rust_jobs SET due_at=0 WHERE kind='image-build'")
            .execute(&control.pool)
            .await
            .unwrap();
        match control.work_once(provider, None).await {
            Ok(false) => return,
            Ok(true) => {}
            Err(e) => assert!(
                matches!(e.code, "image_provider_outcome_unknown"),
                "{}",
                e.code
            ),
        }
    }
    panic!("image lifecycle did not complete");
}
#[test]
fn configuration_matches_reference_json_order() {
    let actual = configuration_digest(
        "update",
        "rust-v1",
        &[
            json!({"project_id":"b","configuration_digest":"two"}),
            json!({"project_id":"a","configuration_digest":"one"}),
        ],
    )
    .unwrap();
    let expected = r#"{"mode":"update","templateVersion":"rust-v1","codexAuthDeliveryVersion":0,"providerAuthDeliveryVersion":1,"repositories":[{"projectId":"a","configurationDigest":"one"},{"projectId":"b","configurationDigest":"two"}]}"#;
    assert_eq!(actual, format!("{:x}", Sha256::digest(expected.as_bytes())));
}
#[tokio::test]
#[ignore = "requires isolated PostgreSQL"]
async fn image_build_retries_verifies_promotes_and_preserves_last_ready() {
    let url = std::env::var("ZUSE_CLOUD_TEST_DATABASE_URL").expect("isolated PostgreSQL required");
    assert!(matches!(
        reqwest::Url::parse(&url).unwrap().host_str(),
        Some("127.0.0.1" | "localhost" | "::1")
    ));
    let admin = PgPool::connect(&url).await.unwrap();
    let schema = format!("images_{}", uuid::Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&admin)
        .await
        .unwrap();
    let copy = schema.clone();
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(8)
        .after_connect(move |conn, _| {
            let schema = copy.clone();
            Box::pin(async move {
                sqlx::query(&format!("SET search_path={schema}"))
                    .execute(conn)
                    .await?;
                Ok(())
            })
        })
        .connect(&url)
        .await
        .unwrap();
    for sql in [
        include_str!("../../tests/fixtures/legacy-catalog.sql"),
        include_str!("../../tests/fixtures/projects.sql"),
        crate::jobs::MIGRATION,
        MIGRATION,
        include_str!("../../tests/fixtures/settlement.sql")
            .split_once('\n')
            .unwrap()
            .1,
    ] {
        sqlx::raw_sql(sql).execute(&pool).await.unwrap();
    }
    sqlx::raw_sql("DROP TABLE api_cloud_snapshots")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::raw_sql(include_str!("../../tests/fixtures/snapshot-storage.sql"))
        .execute(&pool)
        .await
        .unwrap();
    sqlx::raw_sql("INSERT INTO zuse_rust_control_owners VALUES('project-catalog','user-alice','rust',1,0),('image-catalog','user-alice','rust',1,0),('billing','user-alice','rust',1,0); INSERT INTO api_cloud_billing_periods(period_id,account_id,status,period_start,period_end,base_price_micros,included_provider_cost_micros,markup_basis_points,overage_cap_micros,created_at,updated_at) VALUES('period','user-alice','active',0,9999999999999,20000000,10000000,2000,50000000,0,0)").execute(&pool).await.unwrap();
    sqlx::raw_sql("CREATE TABLE api_provider_price_schedule(provider text,version text,effective_at bigint,base_nano_usd_per_second bigint,cpu_nano_usd_per_second bigint,memory_nano_usd_per_gib_second bigint); INSERT INTO api_provider_price_schedule VALUES('boxd','original',0,0,22000,3000)").execute(&pool).await.unwrap();
    for id in ["a", "b"] {
        sqlx::query("INSERT INTO api_cloud_projects(project_id,account_id,repository_identity,repository_url,display_name,default_branch,visibility,git_connection_kind,configuration_digest,state,idempotency_key,created_at,updated_at) VALUES($1,'user-alice',$2,$3,$1,'main','public','github-app','digest','connected',$1,0,0)").bind(id).bind(format!("github.com/owner/{id}")).bind(format!("https://github.com/owner/{id}.git")).execute(&pool).await.unwrap();
    }
    let control = ImageControl::from_pool(pool.clone(), vec!["user-alice".into()], config());
    // Repository-baking builds are retired unless a deployment opts back in.
    let retired = ImageControl::from_pool(
        pool.clone(),
        vec!["user-alice".into()],
        ImageConfig {
            legacy_builds: false,
            ..config()
        },
    );
    assert_eq!(
        retired
            .admit(&access(), request("retired"), None, now())
            .await
            .unwrap_err()
            .code,
        "cloud_image_builds_retired"
    );
    assert_eq!(
        http(
            &Store::from_pool(pool.clone()).with_image_control(retired),
            "POST",
            "/v1/cloud/image/build",
            None,
            Some(json!({"mode":"update","idempotencyKey":"retired"}))
        )
        .await
        .0,
        axum::http::StatusCode::CONFLICT
    );
    let store = Store::from_pool(pool.clone()).with_image_control(control.clone());
    let provider = Provider::default();
    let empty = store.image_status("user-alice", false).await.unwrap();
    assert_eq!(empty["state"], "not-built");
    assert!(provider.0.lock().unwrap().events.is_empty());
    assert_eq!(
        http(
            &store,
            "POST",
            "/v1/cloud/image/build",
            Some("organization:org-team"),
            Some(json!({"mode":"update","idempotencyKey":"denied"}))
        )
        .await
        .0,
        axum::http::StatusCode::FORBIDDEN
    );
    let accepted = http(
        &store,
        "POST",
        "/v1/cloud/image/build",
        None,
        Some(json!({"mode":"update","idempotencyKey":"one"})),
    )
    .await;
    assert_eq!(
        accepted.0,
        axum::http::StatusCode::ACCEPTED,
        "{}",
        accepted.1
    );
    assert_eq!(accepted.1["state"], "building");
    let owner = access();
    let (a, b) = tokio::join!(
        control.admit(&owner, request("one"), None, now()),
        control.admit(&owner, request("one"), None, now())
    );
    a.unwrap();
    b.unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM zuse_rust_image_operations")
            .fetch_one(&pool)
            .await
            .unwrap(),
        1
    );
    {
        let mut s = provider.0.lock().unwrap();
        s.lost_create = true;
        s.lost_capture = true;
    }
    drain(&control, &provider).await;
    let ready = store.image_status("user-alice", true).await.unwrap();
    assert_eq!(ready["state"], "ready");
    let mut original = ready["generation"].clone();
    assert_eq!(ready["repositories"].as_array().unwrap().len(), 2);
    let build: Value = sqlx::query_scalar(
        "SELECT to_jsonb(b) FROM api_cloud_project_builds b WHERE state='ready'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(build["settings"]["rustRuntimeRepositoryLayouts"]["b"]["cachePath"].is_string());
    let reserved: i64 = sqlx::query_scalar(
        "SELECT provider_cost_micros FROM api_cloud_billing_reservations WHERE resource_id=$1",
    )
    .bind(original.as_str())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(reserved > 0);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM api_cloud_billing_ledger")
            .fetch_one(&pool)
            .await
            .unwrap(),
        0
    );
    let launches = store.launch_projects("user-alice").await.unwrap();
    assert_eq!(launches.len(), 2);
    assert!(
        launches.iter().all(|p| p["prepared"] == true),
        "{launches:?}"
    );
    {
        let s = provider.0.lock().unwrap();
        assert!(s.machines.is_empty());
        assert_eq!(
            s.events
                .iter()
                .filter(|e| e.starts_with("allocate:"))
                .count(),
            2
        );
        assert_eq!(s.events.iter().filter(|e| *e == "capture").count(), 1);
        assert_eq!(s.snapshots.len(), 1);
        let verify = s
            .events
            .iter()
            .position(|e| e == "verify-cache-readback")
            .unwrap();
        let destroy = s
            .events
            .iter()
            .position(|e| e.starts_with("destroy:"))
            .unwrap();
        assert!(verify < destroy);
    }
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM api_cloud_runtime_observations")
            .fetch_one(&pool)
            .await
            .unwrap(),
        2
    );
    // A successful rebuild atomically retains the new image and retires the old storage.
    let previous = original.clone();
    control
        .admit(&owner, request("replacement"), None, now())
        .await
        .unwrap();
    drain(&control, &provider).await;
    let replacement = store.image_status("user-alice", true).await.unwrap();
    assert_eq!(replacement["state"], "ready");
    original = replacement["generation"].clone();
    assert_ne!(previous, original);
    let old_record: Value =
        sqlx::query_scalar("SELECT record FROM api_cloud_snapshots WHERE build_id=$1")
            .bind(previous.as_str())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(old_record["state"], "deleted");
    assert_eq!(old_record["deletionReason"], "superseded");
    assert!(old_record["stoppedAtMs"].is_i64());
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM api_cloud_snapshots WHERE state='retained'"
        )
        .fetch_one(&pool)
        .await
        .unwrap(),
        1
    );
    assert_eq!(provider.0.lock().unwrap().snapshots.len(), 1);
    // Corrupt read-back cannot replace a usable generation or retain failed machines.
    control
        .admit(&owner, request("bad"), None, now())
        .await
        .unwrap();
    provider.0.lock().unwrap().corrupt = true;
    drain(&control, &provider).await;
    let failed = store.image_status("user-alice", true).await.unwrap();
    assert_eq!(failed["state"], "failed");
    assert_eq!(failed["generation"], original);
    assert_eq!(failed["errorCode"], "image_manifest_mismatch");
    {
        let s = provider.0.lock().unwrap();
        assert!(s.machines.is_empty());
        assert_eq!(s.snapshots.len(), 1);
    }
    // Image ownership cannot confer billing reservation ownership.
    sqlx::query(
        "UPDATE zuse_rust_control_owners SET backend='legacy' WHERE resource_kind='billing'",
    )
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        control
            .admit(&owner, request("billing-fenced"), None, now())
            .await
            .err()
            .unwrap()
            .code,
        "cloud_image_billing_owner_not_rust"
    );
    sqlx::query("UPDATE zuse_rust_control_owners SET backend='rust' WHERE resource_kind='billing'")
        .execute(&pool)
        .await
        .unwrap();
    // Owner transfer fences queued work before any provider action.
    control
        .admit(&owner, request("fenced"), None, now())
        .await
        .unwrap();
    sqlx::query("UPDATE zuse_rust_control_owners SET backend='legacy' WHERE resource_kind='image-build' AND resource_id=(SELECT build_id FROM api_cloud_project_builds WHERE state='queued')").execute(&pool).await.unwrap();
    assert!(!control.work_once(&provider, None).await.unwrap());
    let bob = Access {
        actor: Principal {
            account_id: "user-bob".into(),
        },
        scope: crate::access::Scope::Personal,
        owner_id: "user-bob".into(),
        membership: None,
    };
    assert_eq!(
        control
            .admit(&bob, request("foreign"), None, now())
            .await
            .err()
            .unwrap()
            .code,
        "cloud_image_writes_disabled"
    );
    assert_eq!(
        store.image_status("user-bob", false).await.unwrap()["state"],
        "not-built"
    );
    // Catalog reads do not contain private pinned configuration or build logs for members.
    sqlx::query("UPDATE api_cloud_project_builds SET log_text='PRIVATE-BUILD-LOG'")
        .execute(&pool)
        .await
        .unwrap();
    assert!(
        !store
            .image_status("user-alice", false)
            .await
            .unwrap()
            .to_string()
            .contains("PRIVATE-BUILD-LOG")
    );
    let record: String =
        sqlx::query_scalar("SELECT string_agg(record::text,'') FROM zuse_rust_image_operations")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(!record.contains("PRIVATE-BUILD-LOG"));
    // A failed-build history window must not hide the older usable generation.
    sqlx::query("INSERT INTO api_cloud_project_builds(build_id,project_id,account_id,provider,template_version,configuration_digest,state,idempotency_key,next_action_at,revision,created_at,updated_at) SELECT 'history-'||n,'a','user-alice','boxd','rust-v1','digest','failed','history-'||n,0,0,$1+n,$1+n FROM generate_series(1,13) n").bind(now()+1000).execute(&pool).await.unwrap();
    let history = store.image_status("user-alice", false).await.unwrap();
    assert_eq!(history["generation"], original);
    assert_eq!(history["builds"].as_array().unwrap().len(), 12);
    assert!(
        history["builds"]
            .as_array()
            .unwrap()
            .iter()
            .all(|b| b["state"] == "failed")
    );
    if std::env::var_os("ZUSE_CLOUD_IMAGES_UI_FIXTURE_FILE").is_none() {
        let image = store.image_status("user-alice", true).await.unwrap();
        let id = image["storage"]["snapshotId"].as_str().unwrap().to_owned();
        assert_eq!(
            http(
                &store,
                "POST",
                "/v1/cloud/image/delete",
                Some("organization:org-team"),
                Some(json!({"snapshotId":id}))
            )
            .await
            .0,
            axum::http::StatusCode::FORBIDDEN
        );
        assert_eq!(
            control
                .delete_image("user-alice", "foreign-snapshot", now())
                .await
                .err()
                .unwrap()
                .code,
            "cloud_snapshot_not_found"
        );
        assert_eq!(
            control
                .delete_image("user-alice", &id, now())
                .await
                .err()
                .unwrap()
                .code,
            "cloud_image_build_in_progress"
        );
        sqlx::query("UPDATE api_cloud_project_builds SET state='failed' WHERE state='queued'")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("UPDATE api_cloud_project_builds SET settings=settings||'{\"providerConnectionId\":\"customer-provider\"}'::jsonb WHERE build_id=$1").bind(original.as_str()).execute(&pool).await.unwrap();
        assert_eq!(
            control
                .delete_image("user-alice", &id, now())
                .await
                .err()
                .unwrap()
                .code,
            "cloud_image_provider_connection_required"
        );
        sqlx::query("UPDATE api_cloud_project_builds SET settings=settings-'providerConnectionId' WHERE build_id=$1").bind(original.as_str()).execute(&pool).await.unwrap();
        sqlx::raw_sql(crate::runtime::allocation::MIGRATION)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO zuse_rust_allocations(operation_id,workspace_id,generation,snapshot_id,machine_name,storage_incarnation_id,boot_expires_at,phase,created_at,updated_at) VALUES('held-allocation','ws-alice',1,$1,'held-source','original-storage',9999999999999,'creating',0,0)").bind(&id).execute(&pool).await.unwrap();
        assert_eq!(
            control
                .delete_image("user-alice", &id, now())
                .await
                .err()
                .unwrap()
                .code,
            "cloud_snapshot_in_use"
        );
        sqlx::query("UPDATE zuse_rust_allocations SET machine_id='retained-original-machine',phase='allocated'").execute(&pool).await.unwrap();
        let retained: Value = sqlx::query_scalar(
            "SELECT to_jsonb(w) FROM api_cloud_workspaces w WHERE workspace_id='ws-alice'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        let start = now() - 3_600_000;
        sqlx::query("UPDATE api_cloud_snapshots SET record=record||$2 WHERE snapshot_id=$1")
            .bind(&id)
            .bind(json!({"retainedAtMs":start,"checkpointAtMs":start,"remainder":123}))
            .execute(&pool)
            .await
            .unwrap();
        // A concurrent admission intent holds the shared source fence through commit.
        let mut admission = pool.begin().await.unwrap();
        super::deletion::snapshot_read_guard(&mut admission, "user-alice", Some(&id))
            .await
            .unwrap();
        let copy = control.clone();
        let deleting = id.clone();
        let cutoff = now();
        let mut task =
            tokio::spawn(async move { copy.delete_image("user-alice", &deleting, cutoff).await });
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(50), &mut task)
                .await
                .is_err()
        );
        admission.commit().await.unwrap();
        task.await.unwrap().unwrap();
        let pending = store.image_status("user-alice", true).await.unwrap();
        assert_eq!(pending["storage"]["state"], "deleting");
        assert!(pending.get("generation").is_none());
        assert!(
            store
                .launch_projects("user-alice")
                .await
                .unwrap()
                .iter()
                .all(|p| p["prepared"] == false)
        );
        let mut new_admission = pool.begin().await.unwrap();
        assert_eq!(
            super::deletion::snapshot_read_guard(&mut new_admission, "user-alice", Some(&id))
                .await
                .err()
                .unwrap()
                .code,
            "cloud_snapshot_unavailable"
        );
        new_admission.rollback().await.unwrap();
        control
            .admit(&owner, request("build-during-cleanup"), None, now())
            .await
            .unwrap();
        provider.0.lock().unwrap().corrupt = false;
        provider.0.lock().unwrap().unavailable_delete = true;
        assert!(control.work_once(&provider, None).await.unwrap());
        let phase:String=sqlx::query_scalar("SELECT record->>'phase' FROM zuse_rust_image_operations WHERE build_id=(SELECT build_id FROM api_cloud_project_builds WHERE idempotency_key='account-image:update:build-during-cleanup')").fetch_one(&pool).await.unwrap();
        assert_eq!(
            phase, "prepare",
            "Cleanup errors must not starve new image builds"
        );
        let failed = store.image_status("user-alice", true).await.unwrap();
        assert_eq!(failed["storage"]["state"], "deleting");
        assert_eq!(failed["storage"]["lastError"], "image_provider_unavailable");
        assert_eq!(provider.0.lock().unwrap().snapshots.len(), 1);
        {
            let mut state = provider.0.lock().unwrap();
            state.unavailable_delete = false;
            state.lost_delete = true;
        }
        sqlx::query("UPDATE api_cloud_snapshots SET record=record||'{\"nextAttemptAtMs\":0}'::jsonb WHERE snapshot_id=$1").bind(&id).execute(&pool).await.unwrap();
        assert!(control.work_once(&provider, None).await.unwrap());
        let mutations = provider
            .0
            .lock()
            .unwrap()
            .events
            .iter()
            .filter(|e| e.as_str() == "delete-failed-snapshot")
            .count();
        sqlx::query("UPDATE api_cloud_snapshots SET record=record||'{\"nextAttemptAtMs\":0}'::jsonb WHERE snapshot_id=$1").bind(&id).execute(&pool).await.unwrap();
        assert!(control.work_once(&provider, None).await.unwrap());
        assert_eq!(
            provider
                .0
                .lock()
                .unwrap()
                .events
                .iter()
                .filter(|e| e.as_str() == "delete-failed-snapshot")
                .count(),
            mutations
        );
        assert!(provider.0.lock().unwrap().snapshots.is_empty());
        control
            .delete_image("user-alice", &id, now() + 1)
            .await
            .unwrap();
        let record: Value =
            sqlx::query_scalar("SELECT record FROM api_cloud_snapshots WHERE snapshot_id=$1")
                .bind(&id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(record["state"], "deleted");
        assert_eq!(record["stoppedAtMs"], cutoff);
        assert_eq!(record["checkpointAtMs"], start);
        assert_eq!(record["remainder"], 123);
        assert!(
            store
                .image_status("user-alice", false)
                .await
                .unwrap()
                .get("storage")
                .is_none()
        );
        let after: Value = sqlx::query_scalar(
            "SELECT to_jsonb(w) FROM api_cloud_workspaces w WHERE workspace_id='ws-alice'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(after, retained);
        sqlx::query("ALTER TABLE api_provider_usage_events ADD COLUMN occurred_at bigint")
            .execute(&pool)
            .await
            .unwrap();
        let billing =
            crate::billing::BillingControl::from_pool(pool.clone(), vec!["user-alice".into()]);
        let config = crate::billing::SnapshotSettlementConfig {
            cutover_at_ms: start,
        };
        let report = billing
            .settle_snapshot_storage_once(config, now() + 1000)
            .await
            .unwrap();
        assert!(report.errors.is_empty(), "{:?}", report.errors);
        let cost:i64=sqlx::query_scalar("SELECT COALESCE(SUM(provider_cost_micros),0)::bigint FROM api_cloud_billing_usage WHERE resource_kind='snapshot' AND resource_id=$1").bind(&id).fetch_one(&pool).await.unwrap();
        assert_eq!(
            cost,
            ((i128::from(cutoff - start) * 1_700_000 + 123) / 2_592_000_000) as i64
        );
        let ledger: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM api_cloud_billing_ledger")
            .fetch_one(&pool)
            .await
            .unwrap();
        billing
            .settle_snapshot_storage_once(config, now() + 2000)
            .await
            .unwrap();
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM api_cloud_billing_ledger")
                .fetch_one(&pool)
                .await
                .unwrap(),
            ledger
        );
        drain(&control, &provider).await;
        assert!(provider.0.lock().unwrap().machines.is_empty());
    }
    if let Ok(path) = std::env::var("ZUSE_CLOUD_IMAGES_UI_FIXTURE_FILE") {
        let retained: Value = sqlx::query_scalar(
            "SELECT to_jsonb(w) FROM api_cloud_workspaces w WHERE workspace_id='ws-alice'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        sqlx::query("DELETE FROM api_cloud_project_builds WHERE build_id LIKE 'history-%'")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("UPDATE api_cloud_project_builds SET state='failed' WHERE state='queued'")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::raw_sql("CREATE TABLE api_cloud_billing_meter_reconciliations(period_id text,provider text,created_at bigint)").execute(&pool).await.unwrap();
        provider.0.lock().unwrap().corrupt = false;
        let provider = Arc::new(provider);
        let fixture = axum::Router::new().route(
            "/catalog",
            axum::routing::get(|| async {
                axum::Json(
                    json!({"configured":false,"user":null,"installations":[],"repositories":[]}),
                )
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let catalog = format!("http://{}/catalog", listener.local_addr().unwrap());
        let authority = tokio::spawn(async move { axum::serve(listener, fixture).await.unwrap() });
        let mut github = crate::github::LegacyGithub::new("https://api.zuse.sh").unwrap();
        github.test_url(catalog.parse().unwrap());
        let app = crate::routes::router(
            crate::routes::AppState {
                store: store.clone().with_github_authority(github),
                identity: Arc::new(Identity),
                organizations_enabled: true,
                organization_ids: Arc::new(vec!["org-team".into()]),
                runtime: None,
                gateway: Arc::new(crate::gateway::Hub::default()),
            },
            vec![],
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let worker_control = control.clone();
        let worker_provider = provider.clone();
        let worker = tokio::spawn(async move {
            loop {
                worker_control
                    .work_once(&*worker_provider, None)
                    .await
                    .unwrap();
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            }
        });
        let file = std::path::Path::new(&path);
        std::fs::write(file, format!("http://{address}")).unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(900), async {
            while !file.with_extension("done").exists() {
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
        })
        .await
        .expect("Image UI acceptance timed out");
        worker.abort();
        server.abort();
        authority.abort();
        let after: Value = sqlx::query_scalar(
            "SELECT to_jsonb(w) FROM api_cloud_workspaces w WHERE workspace_id='ws-alice'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(after, retained);
        assert!(provider.0.lock().unwrap().machines.is_empty());
    }
    pool.close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .unwrap();
    admin.close().await;
}
