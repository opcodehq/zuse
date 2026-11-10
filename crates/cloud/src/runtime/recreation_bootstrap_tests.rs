use super::*;
use axum::{Json, Router, http::HeaderMap, routing::post};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD as B64};
use ed25519_dalek::SigningKey;
use std::sync::atomic::{AtomicUsize, Ordering};
use zuse_cloud_runtime::{Authority, Identity, IdentityBinding};
fn clock_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}
fn bearer(headers: &HeaderMap) -> &str {
    headers["authorization"]
        .to_str()
        .unwrap()
        .strip_prefix("Bearer ")
        .unwrap()
}
#[tokio::test]
#[ignore = "requires explicit isolated localhost PostgreSQL"]
async fn recreation_bootstrap_reaches_real_guest_storage_enrollment_and_cached_identity_stays_unchanged()
 {
    let url = env::var("ZUSE_CLOUD_TEST_DATABASE_URL").expect("isolated PostgreSQL");
    assert!(matches!(
        reqwest::Url::parse(&url).unwrap().host_str(),
        Some("localhost" | "127.0.0.1" | "::1")
    ));
    let admin = PgPool::connect(&url).await.unwrap();
    let schema = format!("recreation_boot_{}", uuid::Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&admin)
        .await
        .unwrap();
    let search = schema.clone();
    let pool = PgPoolOptions::new()
        .max_connections(5)
        .after_connect(move |c, _| {
            let search = search.clone();
            Box::pin(async move {
                sqlx::query(&format!("SET search_path={search}"))
                    .execute(c)
                    .await?;
                Ok(())
            })
        })
        .connect(&url)
        .await
        .unwrap();
    sqlx::raw_sql(include_str!("../../tests/fixtures/legacy-catalog.sql"))
        .execute(&pool)
        .await
        .unwrap();
    sqlx::raw_sql(crate::jobs::MIGRATION)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("ALTER TABLE api_cloud_workspaces ADD COLUMN managed_release_id text")
        .execute(&pool)
        .await
        .unwrap();
    let now = clock_ms();
    sqlx::query("INSERT INTO zuse_rust_control_owners VALUES('workspace','ws-bob','rust',1,$1)")
        .bind(now)
        .execute(&pool)
        .await
        .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let mint = SigningKey::from_bytes(&[81; 32]);
    let jwk = json!({"kty":"OKP","crv":"Ed25519","d":B64.encode([81;32]),"x":B64.encode(mint.verifying_key().as_bytes())});
    let control = Arc::new(RuntimeControl::from_pool(
        pool.clone(),
        MasterKey::parse(&B64.encode([82; 32])).unwrap(),
        TicketSigner::from_jwk(&jwk.to_string(), &origin).unwrap(),
    ));
    let calls = Arc::new(AtomicUsize::new(0));
    let count = calls.clone();
    let boot_control = control.clone();
    let ack_control = control.clone();
    let storage_control = control.clone();
    let app=Router::new()
        .route("/v1/cloud/workspaces/ws-bob/runtime/bootstrap",post(move|headers:HeaderMap,Json(body):Json<BootstrapRequest>|{let control=boot_control.clone();let count=count.clone();async move {count.fetch_add(1,Ordering::SeqCst);Ok::<_,ApiError>(Json(control.bootstrap("ws-bob",bearer(&headers),body,clock_ms()).await?))}}))
        .route("/v1/cloud/workspaces/ws-bob/runtime/bootstrap/ack",post(move|headers:HeaderMap,Json(body):Json<BootstrapAck>|{let control=ack_control.clone();async move {control.acknowledge("ws-bob",bearer(&headers),body,clock_ms()).await?;Ok::<_,ApiError>(Json(json!({"acknowledged":true})))}}))
        .route("/v1/cloud/workspaces/ws-bob/runtime/storage/enroll",post(move|headers:HeaderMap,Json(body):Json<zuse_protocol::cloud::CloudStorageEnrollment>|{let control=storage_control.clone();async move {Ok::<_,ApiError>(Json(control.enroll_storage("ws-bob",bearer(&headers),body,clock_ms()).await?))}}));
    let _server = tokio_util::task::AbortOnDropHandle::new(tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap()
    }));
    let authority = Authority::new(&origin).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let binding = IdentityBinding {
        workspace: "ws-bob".into(),
        api_url: authority.origin(),
        incarnation: "disk-bob".into(),
        generation: 1,
    };
    let mut identity = Identity::open(directory.path(), &binding, Some("fixture-boot")).unwrap();
    for (name, version, advertises, managed) in [
        ("supported", true, true, true),
        ("old-guest", true, false, true),
        ("missing-image-version", false, true, true),
        ("unmanaged", true, true, false),
    ] {
        let config = json!({"runtimeGeneration":1,"gatewayEpoch":1,"runtimeRpcProtocol":zuse_protocol::cloud_gateway::RUST_RPC,"providerAuthMode":"broker-v1","managedReleaseId":"verified-release","rustRecreationVersion":if version {json!(1)}else{Value::Null},"rustRuntimeStorage":{"format":"zuse-rust-engine-v1","storageIncarnationId":"disk-bob"}});
        sqlx::query("UPDATE api_cloud_workspaces SET request_config=$1,managed_release_id=$2,runtime_boot_token_hash=$3,runtime_boot_token_expires_at=$4,status_code='agent-starting' WHERE workspace_id='ws-bob'").bind(config).bind(managed.then_some("verified-release")).bind(hash("fixture-boot")).bind(clock_ms()+120000).execute(&pool).await.unwrap();
        let mut request = identity.request();
        if !advertises {
            request
                .capabilities
                .as_mut()
                .unwrap()
                .retain(|c| c != zuse_protocol::cloud_lifecycle::RECREATION_CAPABILITY);
        }
        // The actual production bootstrap feeds the actual guest storage client;
        // no test substitutes its conditional capability construction.
        let response = control
            .bootstrap("ws-bob", "fixture-boot", request, clock_ms())
            .await
            .unwrap();
        let enabled = version && advertises && managed;
        assert_eq!(response["rustRecreationVersion"] == 1, enabled, "{name}");
        let boot: zuse_protocol::cloud::CloudRuntimeBootstrap =
            serde_json::from_value(response).unwrap();
        let enrollment = authority.prepare_storage(&boot).await.unwrap();
        authority.commit_storage(&boot, &enrollment).await.unwrap();
        let row: Value = sqlx::query_scalar(
            "SELECT to_jsonb(w) FROM api_cloud_workspaces w WHERE workspace_id='ws-bob'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(storage_enrolled(&row));
        assert_eq!(
            row["request_config"]["rustStorageEnrollment"]["capabilities"]
                .as_array()
                .unwrap()
                .iter()
                .any(|c| c == zuse_protocol::cloud_lifecycle::RECREATION_CAPABILITY),
            enabled,
            "{name}"
        );
        if name == "supported" {
            // Model an old saved bootstrap without this field, using the public
            // identity persistence API only in this isolated fixture.
            let mut cached = boot.clone();
            cached.extra.remove("rustRecreationVersion");
            identity.commit_bootstrap(cached, 1).unwrap();
            identity.acknowledge().unwrap();
            let before = calls.load(Ordering::SeqCst);
            let retained = authority
                .enroll(&mut identity, &binding, None)
                .await
                .unwrap();
            assert!(!retained.extra.contains_key("rustRecreationVersion"));
            assert_eq!(
                calls.load(Ordering::SeqCst),
                before,
                "Restart/enroll cannot refresh an already saved bootstrap"
            );
        }
    }
    drop(_server);
    pool.close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .unwrap();
    admin.close().await;
}
