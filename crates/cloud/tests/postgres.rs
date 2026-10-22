//! Explicit opt-in only: these tests create a private schema in a localhost database.
use async_trait::async_trait;
use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use serde_json::{Value, json};
use sqlx::{PgPool, postgres::PgPoolOptions};
use std::sync::Arc;
use tower::ServiceExt;
use uuid::Uuid;
use zuse_cloud::{
    auth::{Identity, Membership, MembershipRole, Principal},
    error::ApiError,
    jobs,
    routes::{AppState, router},
    store::Store,
};

struct TestIdentity;
#[async_trait]
impl Identity for TestIdentity {
    async fn verify(&self, token: &str) -> Result<Principal, ApiError> {
        if !["alice", "bob", "admin", "member", "billing"].contains(&token) {
            return Err(ApiError::unauthorized("invalid_workos_token"));
        }
        Ok(Principal {
            account_id: format!("user-{token}"),
        })
    }
    async fn membership(&self, account: &str, organization: &str) -> Result<Membership, ApiError> {
        let role = account.strip_prefix("user-").unwrap_or("");
        if organization != "org-team" || !["admin", "member", "billing"].contains(&role) {
            return Err(ApiError::forbidden("organization_access_denied"));
        }
        Ok(Membership {
            id: format!("member-{role}"),
            user_id: account.into(),
            organization_id: organization.into(),
            status: "active".into(),
            role: MembershipRole { slug: role.into() },
        })
    }
}

async fn request(
    app: &Router,
    path: &str,
    token: &str,
    scope: Option<&str>,
) -> (StatusCode, Value) {
    let mut builder = Request::builder().uri(path);
    if !token.is_empty() {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }
    if let Some(scope) = scope {
        builder = builder.header("x-zuse-workspace", scope);
    }
    let response = app
        .clone()
        .oneshot(builder.body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.headers()["cache-control"], "no-store");
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

async fn fixture() -> (PgPool, PgPool, String) {
    let url = std::env::var("ZUSE_CLOUD_TEST_DATABASE_URL")
        .expect("Set explicit localhost ZUSE_CLOUD_TEST_DATABASE_URL");
    let parsed = reqwest::Url::parse(&url).unwrap();
    assert!(
        matches!(parsed.host_str(), Some("127.0.0.1" | "localhost" | "::1")),
        "Tests require localhost"
    );
    let admin = PgPool::connect(&url).await.unwrap();
    let schema = format!("cloud_test_{}", Uuid::new_v4().simple());
    sqlx::raw_sql(&format!("CREATE SCHEMA {schema}"))
        .execute(&admin)
        .await
        .unwrap();
    let schema_copy = schema.clone();
    let pool = PgPoolOptions::new()
        .max_connections(5)
        .after_connect(move |connection, _| {
            let schema = schema_copy.clone();
            Box::pin(async move {
                sqlx::query(&format!("SET search_path = {schema}"))
                    .execute(connection)
                    .await?;
                Ok(())
            })
        })
        .connect(&url)
        .await
        .unwrap();
    sqlx::raw_sql(include_str!("fixtures/legacy-catalog.sql"))
        .execute(&pool)
        .await
        .unwrap();
    sqlx::raw_sql(jobs::MIGRATION).execute(&pool).await.unwrap();
    (admin, pool, schema)
}

#[tokio::test]
#[ignore = "requires explicit isolated localhost PostgreSQL"]
async fn catalog_authorization_archive_reads_and_durable_job_recovery() {
    let (admin, pool, schema) = fixture().await;
    let state = AppState {
        store: Store::from_pool(pool.clone()),
        identity: Arc::new(TestIdentity),
        organizations_enabled: true,
        organization_ids: Arc::new(vec!["org-team".into()]),
        runtime: None,
    };
    let app = router(state, vec![]);
    assert_eq!(request(&app, "/readyz", "", None).await.0, StatusCode::OK);
    assert_eq!(
        request(&app, "/v1/cloud/workspaces", "", None).await.0,
        StatusCode::UNAUTHORIZED
    );
    let (status, catalog) = request(&app, "/v1/cloud/workspaces", "alice", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(catalog["workspaces"].as_array().unwrap().len(), 1);
    assert_eq!(catalog["workspaces"][0]["workspaceId"], "ws-alice");
    for secret in [
        "PRIVATE",
        "request_config",
        "runtimeCredentialHash",
        "account_id",
        "machine-alice",
    ] {
        assert!(
            !catalog.to_string().contains(secret),
            "Internal field escaped public projection"
        );
    }
    assert_eq!(
        request(&app, "/v1/cloud/workspaces/ws-bob", "alice", None)
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    let (_, archive) = request(
        &app,
        "/v1/cloud/workspaces/ws-alice/archive-status",
        "alice",
        None,
    )
    .await;
    assert_eq!(archive["retentionDeadline"], 100 + 10 * 86400000_i64);
    assert_eq!(archive["machineRetained"], true);
    assert_eq!(archive["backupVerified"], false);
    assert_eq!(archive["computeRetired"], false);
    let retained: String = sqlx::query_scalar(
        "SELECT provider_sandbox_id FROM api_cloud_workspaces WHERE workspace_id='ws-alice'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(retained, "machine-alice");
    let (_, entitlements) = request(&app, "/v1/billing/entitlements", "alice", None).await;
    assert_eq!(
        entitlements["entitlements"][0]["entitlementId"],
        "ent-alice"
    );
    assert!(!entitlements.to_string().contains("sub-preserved"));
    let org = Some("organization:org-team");
    assert_eq!(
        request(&app, "/v1/cloud/workspaces", "billing", org)
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        request(&app, "/v1/billing/entitlements", "member", org)
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        request(&app, "/v1/billing/entitlements", "billing", org)
            .await
            .0,
        StatusCode::OK
    );
    let (_, member_rows) = request(&app, "/v1/cloud/workspaces", "member", org).await;
    assert_eq!(member_rows["workspaces"].as_array().unwrap().len(), 1);
    assert_eq!(member_rows["workspaces"][0]["workspaceId"], "ws-org-shared");
    assert_eq!(
        request(&app, "/v1/cloud/workspaces/ws-org-private", "member", org)
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        request(&app, "/v1/cloud/workspaces/ws-org-private", "admin", org)
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(
        request(&app, "/v1/cloud/workspaces", "alice", Some("organization:"))
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        request(
            &app,
            "/v1/cloud/workspaces",
            "admin",
            Some("organization:other")
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );

    let mut tx = pool.begin().await.unwrap();
    let job_id = jobs::enqueue(
        &mut tx,
        "archive:one",
        "archive",
        "ws-alice",
        1,
        &json!({"operationId":"one"}),
        100,
    )
    .await
    .unwrap();
    assert_eq!(
        jobs::enqueue(
            &mut tx,
            "archive:one",
            "archive",
            "ws-alice",
            1,
            &json!({"operationId":"one"}),
            101
        )
        .await
        .unwrap(),
        job_id
    );
    assert!(
        jobs::enqueue(
            &mut tx,
            "archive:one",
            "archive",
            "ws-alice",
            1,
            &json!({"operationId":"other"}),
            101
        )
        .await
        .is_err()
    );
    tx.commit().await.unwrap();
    assert!(
        jobs::claim(&pool, "archive", "workspace", 100, 1000)
            .await
            .unwrap()
            .is_none(),
        "Legacy remains owner by default"
    );
    sqlx::query(
        "INSERT INTO zuse_rust_control_owners VALUES ('workspace','ws-alice','rust',1,100)",
    )
    .execute(&pool)
    .await
    .unwrap();
    let (first, second) = tokio::join!(
        jobs::claim(&pool, "archive", "workspace", 100, 1000),
        jobs::claim(&pool, "archive", "workspace", 100, 1000)
    );
    let mut claims = vec![first.unwrap(), second.unwrap()]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    assert_eq!(claims.len(), 1);
    let first = claims.pop().unwrap();
    let recovered = jobs::claim(&pool, "archive", "workspace", 1101, 1000)
        .await
        .unwrap()
        .unwrap();
    assert_ne!(first.lease_token, recovered.lease_token);
    assert!(
        !jobs::finish(&pool, &first, "workspace", 1102)
            .await
            .unwrap()
    );
    assert!(
        jobs::retry(&pool, &recovered, 1102, "provider_unavailable")
            .await
            .unwrap()
    );
    assert!(
        jobs::claim(&pool, "archive", "workspace", 1103, 1000)
            .await
            .unwrap()
            .is_none()
    );
    let retry = jobs::claim(&pool, "archive", "workspace", 10000, 1000)
        .await
        .unwrap()
        .unwrap();
    assert!(
        jobs::finish(&pool, &retry, "workspace", 10001)
            .await
            .unwrap()
    );
    assert!(
        jobs::claim(&pool, "archive", "workspace", 20000, 1000)
            .await
            .unwrap()
            .is_none()
    );

    // Exercise the actual production Store connection, not a parallel test implementation.
    let mut url =
        reqwest::Url::parse(&std::env::var("ZUSE_CLOUD_TEST_DATABASE_URL").unwrap()).unwrap();
    url.query_pairs_mut()
        .append_pair("options", &format!("-csearch_path={schema}"));
    let readonly = Store::connect_read_only(url.as_str()).await.unwrap();
    let report = readonly.migration_report().await.unwrap();
    assert_eq!(report["databaseReadOnly"], true);
    assert_eq!(report["workspaces"]["workspaces"], 4);
    assert_eq!(report["ownershipTransferred"], false);
    drop(readonly);
    pool.close().await;
    // This schema name is generated above; no application or shared tables are dropped.
    sqlx::raw_sql(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .unwrap();
    admin.close().await;
}

async fn runtime_post(app: &Router, path: &str, token: &str, body: &Value) -> (StatusCode, Value) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(path)
                .header("authorization", format!("Bearer {token}"))
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.headers()["cache-control"], "no-store");
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

#[tokio::test]
#[ignore = "requires explicit isolated localhost PostgreSQL"]
async fn runtime_enrollment_preserves_keys_intent_and_fences_retries() {
    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
    use ed25519_dalek::SigningKey;
    use sha2::{Digest, Sha256};
    use zuse_cloud::{crypto::MasterKey, runtime::RuntimeControl, tickets::TicketSigner};
    let (admin, pool, schema) = fixture().await;
    let fixture: Value = serde_json::from_str(include_str!("fixtures/cloud-crypto.json")).unwrap();
    let master = MasterKey::parse(fixture["masterKey"].as_str().unwrap()).unwrap();
    let (_, envelope) = master.create_transcript_key("user-bob", "ws-bob").unwrap();
    let boot = "synthetic-bootstrap-token";
    let hash = |s: &str| -> String {
        Sha256::digest(s.as_bytes())
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    };
    let now = chrono::Utc::now().timestamp_millis();
    sqlx::query("UPDATE api_cloud_workspaces SET runtime_boot_token_hash=$1,runtime_boot_token_expires_at=$2,wrapped_transcript_key=$3,request_config=$4 WHERE workspace_id='ws-bob'")
        .bind(hash(boot)).bind(now+120000).bind(&envelope)
        .bind(json!({"runtimeGeneration":2,"gatewayEpoch":5,"providerAuthMode":"broker-v1"}))
        .execute(&pool).await.unwrap();
    let intent = json!({"commandId":"original-command","turnId":"original-turn","title":"Original chat","agent":"claude","model":"original-model","permissions":[],"runtimeMode":"approval-required","firstMessage":"Keep the original prompt"});
    let ciphertext = master
        .seal(
            b"zuse-cloud-workspace-launch-intent-v1\nuser-bob\nws-bob",
            intent.to_string().as_bytes(),
        )
        .unwrap();
    sqlx::query("INSERT INTO api_cloud_workspace_launch_intents VALUES ('ws-bob','user-bob','chat-bob','session-bob','original-turn','original-command',$1,$2,$3)")
        .bind(ciphertext).bind(now+120000).bind(now).execute(&pool).await.unwrap();
    let mint = SigningKey::from_bytes(&[7; 32]);
    let mint_jwk = json!({"kty":"OKP","crv":"Ed25519","d":URL_SAFE_NO_PAD.encode([7;32]),"x":URL_SAFE_NO_PAD.encode(mint.verifying_key().as_bytes())});
    let guest = SigningKey::from_bytes(&[8; 32]);
    let guest_jwk = json!({"kty":"OKP","crv":"Ed25519","x":URL_SAFE_NO_PAD.encode(guest.verifying_key().as_bytes())});
    let signer = TicketSigner::from_jwk(&mint_jwk.to_string(), "https://api-rust.zuse.sh").unwrap();
    let app = router(
        AppState {
            store: Store::from_pool(pool.clone()),
            identity: Arc::new(TestIdentity),
            organizations_enabled: false,
            organization_ids: Arc::new(vec![]),
            runtime: Some(Arc::new(RuntimeControl::from_pool(
                pool.clone(),
                master,
                signer,
            ))),
        },
        vec![],
    );
    let body = json!({"credentialPublicJwk":fixture["runtimePublicJwk"].to_string(),"signingPublicJwk":guest_jwk.to_string(),"capabilities":["github-execution-v1"]});
    let path = "/v1/cloud/workspaces/ws-bob/runtime/bootstrap";
    assert_eq!(
        runtime_post(&app, path, boot, &body).await.0,
        StatusCode::UNAUTHORIZED
    );
    sqlx::query("INSERT INTO zuse_rust_control_owners VALUES ('workspace','ws-bob','rust',2,$1)")
        .bind(now)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        runtime_post(&app, path, "wrong-token", &body).await.0,
        StatusCode::UNAUTHORIZED
    );
    let (a, b) = tokio::join!(
        runtime_post(&app, path, boot, &body),
        runtime_post(&app, path, boot, &body)
    );
    assert_eq!(a.0, StatusCode::OK, "{}", a.1);
    assert_eq!(b.0, StatusCode::OK, "{}", b.1);
    assert_eq!(
        a.1, b.1,
        "Concurrent retries must return the committed receipt"
    );
    let response = a.1;
    assert_eq!(response["launchIntent"], intent);
    assert_eq!(response["chatId"], "chat-bob");
    assert_eq!(response["initialSessionId"], "session-bob");
    assert_eq!(response["providerAuthMode"], "broker-v1");
    assert_eq!(response["gatewayProtocol"], "zuse-workspace-v2");
    let token = response["runtimeCredential"].as_str().unwrap();
    let gateway_token = response["runtimeGatewayCredential"].as_str().unwrap();
    let public = json!({"kty":"OKP","crv":"Ed25519","x":mint_jwk["x"]});
    let key =
        jsonwebtoken::DecodingKey::from_jwk(&serde_json::from_value(public).unwrap()).unwrap();
    let mut validation = jsonwebtoken::Validation::new(jsonwebtoken::Algorithm::EdDSA);
    validation.set_audience(&["zuse-workspace:ws-bob"]);
    validation.set_issuer(&["https://api-rust.zuse.sh"]);
    let decoded = jsonwebtoken::decode::<Value>(gateway_token, &key, &validation).unwrap();
    assert_eq!(decoded.header.typ.as_deref(), Some("workspace-runtime+jwt"));
    assert_eq!(decoded.claims["generation"], 2);
    assert_eq!(decoded.claims["gatewayEpoch"], 5);
    assert_eq!(decoded.claims["sub"], "user-bob");
    let stored: Value = sqlx::query_scalar(
        "SELECT to_jsonb(w) FROM api_cloud_workspaces w WHERE workspace_id='ws-bob'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(stored["wrapped_transcript_key"], envelope);
    assert_eq!(stored["runtime_credential_hash"], hash(token));
    assert_eq!(stored["revision"], 2);
    assert_eq!(stored["state"], "setup");
    let mut changed = body.clone();
    changed["signingPublicJwk"] =
        json!(json!({"kty":"OKP","crv":"Ed25519","x":mint_jwk["x"]}).to_string());
    assert_eq!(
        runtime_post(&app, path, boot, &changed).await.0,
        StatusCode::UNAUTHORIZED
    );
    let ack_path = format!("{path}/ack");
    assert_eq!(
        runtime_post(
            &app,
            &ack_path,
            token,
            &json!({"runtimeGeneration":1,"gatewayEpoch":5})
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    let ack = json!({"runtimeGeneration":2,"gatewayEpoch":5});
    assert_eq!(
        runtime_post(&app, &ack_path, token, &ack).await.0,
        StatusCode::OK
    );
    assert_eq!(
        runtime_post(&app, &ack_path, token, &ack).await.0,
        StatusCode::OK
    );
    let stored: Value = sqlx::query_scalar(
        "SELECT to_jsonb(w) FROM api_cloud_workspaces w WHERE workspace_id='ws-bob'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(stored["runtime_boot_token_hash"].is_null());
    assert!(stored["runtime_boot_token_expires_at"].is_null());
    assert_eq!(
        stored["revision"], 3,
        "Repeated ACK must not advance revision"
    );
    assert_eq!(
        runtime_post(&app, path, boot, &body).await.0,
        StatusCode::UNAUTHORIZED
    );
    sqlx::query("UPDATE zuse_rust_control_owners SET generation=3 WHERE resource_id='ws-bob'")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        runtime_post(&app, &ack_path, token, &ack).await.0,
        StatusCode::UNAUTHORIZED
    );
    // Failed first enrollment must roll back key creation and leave no accepted receipt.
    sqlx::query("UPDATE zuse_rust_control_owners SET generation=2 WHERE resource_id='ws-bob'")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM api_cloud_workspace_launch_intents WHERE workspace_id='ws-bob'")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE api_cloud_workspaces SET runtime_boot_token_hash=$1,runtime_boot_token_expires_at=$2,wrapped_transcript_key=NULL,runtime_credential_hash=NULL,request_config=$3 WHERE workspace_id='ws-bob'")
        .bind(hash(boot)).bind(now+120000).bind(json!({"runtimeGeneration":2,"gatewayEpoch":5})).execute(&pool).await.unwrap();
    assert_eq!(
        runtime_post(&app, path, boot, &body).await.0,
        StatusCode::CONFLICT
    );
    let stored: Value = sqlx::query_scalar(
        "SELECT to_jsonb(w) FROM api_cloud_workspaces w WHERE workspace_id='ws-bob'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(stored["wrapped_transcript_key"].is_null());
    assert!(stored["runtime_credential_hash"].is_null());
    assert!(stored["request_config"]["runtimeBootstrapReceipt"].is_null());
    pool.close().await;
    sqlx::raw_sql(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .unwrap();
    admin.close().await;
}
