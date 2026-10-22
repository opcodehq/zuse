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
    sqlx::raw_sql(zuse_cloud::gateway::MIGRATION)
        .execute(&pool)
        .await
        .unwrap();
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
        gateway: Arc::new(zuse_cloud::gateway::Hub::default()),
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
            gateway: Arc::new(zuse_cloud::gateway::Hub::default()),
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
    let mut incompatible = body.clone();
    incompatible["rpcProtocol"] = json!(zuse_protocol::cloud_gateway::RUST_RPC);
    assert_eq!(
        runtime_post(&app, path, boot, &incompatible).await.0,
        StatusCode::CONFLICT
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
    // A fresh signature recovers an expired bearer, while a lost renewal response
    // can be retrieved using its original request ID and previous bearer.
    use ed25519_dalek::pkcs8::EncodePrivateKey;
    let proof_key =
        jsonwebtoken::EncodingKey::from_ed_der(guest.to_pkcs8_der().unwrap().as_bytes());
    let proof = |request_id: &str, seconds: i64| {
        let mut header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::EdDSA);
        header.typ = Some("workspace-runtime-renewal+jwt".into());
        jsonwebtoken::encode(&header, &json!({"workspaceId":"ws-bob","requestId":request_id,"generation":2,"gatewayEpoch":5,"aud":"https://api-rust.zuse.sh","iat":seconds,"exp":seconds+60}), &proof_key).unwrap()
    };
    let renew_path = "/v1/cloud/workspaces/ws-bob/runtime/credentials/renew";
    let renewal = json!({"requestId":"renew-original","proof":proof("renew-original",now/1000)});
    assert_eq!(
        runtime_post(
            &app,
            renew_path,
            token,
            &json!({"requestId":"renew-original","proof":"invalid"})
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    sqlx::query("UPDATE api_cloud_workspaces SET request_config=jsonb_set(request_config,'{runtimeCredentialExpiresAtMs}',$1) WHERE workspace_id='ws-bob'")
        .bind(json!(now-1000)).execute(&pool).await.unwrap();
    let (first, retry) = tokio::join!(
        runtime_post(&app, renew_path, token, &renewal),
        runtime_post(&app, renew_path, token, &renewal)
    );
    assert_eq!(first.0, StatusCode::OK, "{}", first.1);
    assert_eq!(first, retry);
    assert_eq!(
        first.1["runtimeCredential"],
        format!(
            "workspace_runtime_{}",
            hash(&format!("{token}:ws-bob:renew-original"))
        )
    );
    let renewed_token = first.1["runtimeCredential"].as_str().unwrap();
    assert_eq!(
        runtime_post(&app, &ack_path, renewed_token, &ack).await.0,
        StatusCode::OK
    );

    sqlx::query("UPDATE api_cloud_workspaces SET request_config=jsonb_set(jsonb_set(request_config,'{runtimeCredentialExpiresAtMs}',$1),'{runtimeCredentialRenewal,expiresAtMs}',$1) WHERE workspace_id='ws-bob'")
        .bind(json!(now-1000)).execute(&pool).await.unwrap();
    let recovered = runtime_post(&app, renew_path, token, &renewal).await;
    assert_eq!(recovered.0, StatusCode::OK, "{}", recovered.1);
    assert_eq!(recovered.1["runtimeCredential"], renewed_token);
    assert!(recovered.1["expiresAt"].as_i64().unwrap() > now);

    let next_renewal = json!({"requestId":"renew-next","proof":proof("renew-next",now/1000)});
    assert_eq!(
        runtime_post(&app, renew_path, token, &next_renewal).await.0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        runtime_post(
            &app,
            renew_path,
            renewed_token,
            &json!({"requestId":"renew-next","proof":proof("renew-wrong",now/1000)})
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        runtime_post(
            &app,
            renew_path,
            renewed_token,
            &json!({"requestId":"renew-next","proof":proof("renew-next",now/1000-180)})
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    sqlx::query("UPDATE api_cloud_workspaces SET request_config=jsonb_set(request_config,'{cloudMailboxLifecyclePending}','{\"action\":\"delete\"}') WHERE workspace_id='ws-bob'").execute(&pool).await.unwrap();
    assert_eq!(
        runtime_post(&app, renew_path, renewed_token, &next_renewal)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    sqlx::query("UPDATE api_cloud_workspaces SET request_config=request_config-'cloudMailboxLifecyclePending' WHERE workspace_id='ws-bob'").execute(&pool).await.unwrap();
    sqlx::query("UPDATE zuse_rust_control_owners SET generation=3 WHERE resource_id='ws-bob'")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        runtime_post(&app, &ack_path, token, &ack).await.0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        runtime_post(&app, renew_path, renewed_token, &next_renewal)
            .await
            .0,
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
    sqlx::query("UPDATE api_cloud_workspaces SET state='archived' WHERE workspace_id='ws-bob'")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        runtime_post(&app, path, boot, &body).await.0,
        StatusCode::UNAUTHORIZED
    );
    sqlx::query("UPDATE api_cloud_workspaces SET state='ready',request_config=jsonb_set(request_config,'{cloudMailboxLifecycleDelivered}','{\"action\":\"delete\"}') WHERE workspace_id='ws-bob'").execute(&pool).await.unwrap();
    assert_eq!(
        runtime_post(&app, path, boot, &body).await.0,
        StatusCode::UNAUTHORIZED
    );
    sqlx::query("UPDATE api_cloud_workspaces SET request_config=request_config-'cloudMailboxLifecycleDelivered' WHERE workspace_id='ws-bob'").execute(&pool).await.unwrap();
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
    // Existing unsigned guests retain the original valid-bearer renewal path;
    // an expired bearer without a registered signing identity cannot recover.
    sqlx::query("UPDATE api_cloud_workspaces SET runtime_credential_hash=$1,request_config=$2 WHERE workspace_id='ws-bob'")
        .bind(hash("legacy-runtime")).bind(json!({"runtimeGeneration":2,"gatewayEpoch":5,"runtimeCredentialExpiresAtMs":now+120000})).execute(&pool).await.unwrap();
    let legacy = runtime_post(
        &app,
        renew_path,
        "legacy-runtime",
        &json!({"requestId":"legacy-renew","proof":""}),
    )
    .await;
    assert_eq!(legacy.0, StatusCode::OK, "{}", legacy.1);
    sqlx::query("UPDATE api_cloud_workspaces SET request_config=jsonb_set(request_config,'{runtimeCredentialExpiresAtMs}',$1) WHERE workspace_id='ws-bob'")
        .bind(json!(now-1000)).execute(&pool).await.unwrap();
    assert_eq!(
        runtime_post(
            &app,
            renew_path,
            legacy.1["runtimeCredential"].as_str().unwrap(),
            &json!({"requestId":"legacy-expired","proof":""})
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );

    pool.close().await;
    sqlx::raw_sql(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .unwrap();
    admin.close().await;
}

type TestSocket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;
async fn socket_connect(url: &str, protocol: &str, credential: &str) -> TestSocket {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    let mut request = url.into_client_request().unwrap();
    request.headers_mut().insert(
        "sec-websocket-protocol",
        format!("{protocol}, {credential}").parse().unwrap(),
    );
    // These headers are untrusted and must not override ticket-derived authority.
    request
        .headers_mut()
        .insert("x-zuse-gateway-actor", "attacker".parse().unwrap());
    let (socket, response) = tokio_tungstenite::connect_async(request).await.unwrap();
    assert_eq!(response.headers()["sec-websocket-protocol"], protocol);
    socket
}
async fn socket_message(socket: &mut TestSocket) -> tokio_tungstenite::tungstenite::Message {
    use futures::StreamExt;
    tokio::time::timeout(std::time::Duration::from_secs(5), socket.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap()
}
async fn socket_json(socket: &mut TestSocket) -> Value {
    serde_json::from_str(socket_message(socket).await.to_text().unwrap()).unwrap()
}

#[tokio::test]
#[ignore = "requires explicit isolated localhost PostgreSQL"]
async fn gateway_multiplexes_live_sockets_and_fences_runtime_ownership() {
    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
    use ed25519_dalek::SigningKey;
    use futures::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message;
    use zuse_cloud::{crypto::MasterKey, runtime::RuntimeControl, tickets::TicketSigner};
    use zuse_protocol::cloud_gateway::{Direction, Frame, PENDING_PROTOCOL, PROTOCOL, Payload};
    let (admin, pool, schema) = fixture().await;
    let now = chrono::Utc::now().timestamp_millis();
    sqlx::query("INSERT INTO zuse_rust_control_owners VALUES ('workspace','ws-bob','rust',2,$1)")
        .bind(now)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE api_cloud_workspaces SET request_config=$1 WHERE workspace_id='ws-bob'")
        .bind(json!({"runtimeGeneration":2,"gatewayEpoch":5}))
        .execute(&pool)
        .await
        .unwrap();
    let mint = SigningKey::from_bytes(&[9; 32]);
    let jwk=json!({"kty":"OKP","crv":"Ed25519","d":URL_SAFE_NO_PAD.encode([9;32]),"x":URL_SAFE_NO_PAD.encode(mint.verifying_key().as_bytes())}).to_string();
    let signer = TicketSigner::from_jwk(&jwk, "https://api-rust.zuse.sh").unwrap();
    let runtime_ticket = signer
        .runtime("user-bob", "ws-bob", 2, 5, now + 900000)
        .unwrap();
    let wrong_workspace = signer
        .runtime("user-bob", "ws-alice", 2, 5, now + 900000)
        .unwrap();
    let expired = signer
        .runtime("user-bob", "ws-bob", 2, 5, now - 1000)
        .unwrap();
    let control = Arc::new(RuntimeControl::from_pool(
        pool.clone(),
        MasterKey::parse(&URL_SAFE_NO_PAD.encode([10; 32])).unwrap(),
        signer,
    ));
    let app = router(
        AppState {
            store: Store::from_pool(pool.clone()),
            identity: Arc::new(TestIdentity),
            organizations_enabled: false,
            organization_ids: Arc::new(vec![]),
            runtime: Some(control.clone()),
            gateway: Arc::new(zuse_cloud::gateway::Hub::default()),
        },
        vec![],
    );
    let ticket_path = "/v1/cloud/workspaces/ws-bob/gateway/ticket";
    assert_eq!(
        runtime_post(&app, ticket_path, "alice", &json!({})).await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        runtime_post(&app, ticket_path, "bob", &json!({"protocol":"unknown"}))
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    let (status, ticket) = runtime_post(
        &app,
        ticket_path,
        "bob",
        &json!({"protocol":PENDING_PROTOCOL}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{ticket}");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!(
        "ws://{}/v1/cloud/workspaces/ws-bob/gateway",
        listener.local_addr().unwrap()
    );
    let (stop, stopped) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async {
                let _ = stopped.await;
            })
            .await
            .unwrap();
    });
    let credential = ticket["credential"].as_str().unwrap();
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    for (credential, origin, expected) in [
        (&wrong_workspace, None, 401),
        (&expired, None, 401),
        (&runtime_ticket, Some("https://unapproved.example"), 403),
    ] {
        let mut request = url.as_str().into_client_request().unwrap();
        request.headers_mut().insert(
            "sec-websocket-protocol",
            format!("{PROTOCOL}, {credential}").parse().unwrap(),
        );
        if let Some(origin) = origin {
            request
                .headers_mut()
                .insert("origin", origin.parse().unwrap());
        }
        match tokio_tungstenite::connect_async(request).await {
            Err(tokio_tungstenite::tungstenite::Error::Http(response)) => {
                assert_eq!(response.status().as_u16(), expected)
            }
            _ => panic!("Gateway accepted invalid admission authority"),
        }
    }
    let mut client = socket_connect(&url, PENDING_PROTOCOL, credential).await;
    assert_eq!(
        socket_json(&mut client).await,
        json!({"_zuseGateway":"availability","state":"pending"})
    );
    client
        .send(Message::Text("never replay this mutation".into()))
        .await
        .unwrap();
    // Ensure the pending send is processed before the runtime arrives.
    client.send(Message::Ping(vec![1])).await.unwrap();
    assert!(matches!(
        socket_message(&mut client).await,
        Message::Pong(_)
    ));
    let mut runtime = socket_connect(&url, PROTOCOL, &runtime_ticket).await;
    let opened = socket_json(&mut runtime).await;
    assert_eq!(opened["type"], "client.open");
    assert_eq!(opened["actorId"], "user-bob");
    assert_eq!(opened["permission"], "edit");
    let connection = opened["connectionId"].as_str().unwrap();
    assert_eq!(
        socket_json(&mut client).await,
        json!({"_zuseGateway":"availability","state":"available","reset":false})
    );
    assert!(
        control
            .gateway_lease("ws-bob", "competing-process", (2, 5))
            .await
            .is_err()
    );
    let payload = vec![0, 255, 1, 2, 0, 128];
    client.send(Message::Binary(payload.clone())).await.unwrap();
    let frame = Frame::decode(&socket_message(&mut runtime).await.into_data()).unwrap();
    assert_eq!(frame.direction, Direction::Client);
    assert_eq!(frame.connection_id, connection);
    assert_eq!(frame.payload, Payload::Binary(payload));
    runtime
        .send(Message::Binary(
            Frame {
                direction: Direction::Runtime,
                connection_id: connection.into(),
                payload: Payload::Text("original response".into()),
            }
            .encode()
            .unwrap(),
        ))
        .await
        .unwrap();
    assert_eq!(
        socket_message(&mut client).await.to_text().unwrap(),
        "original response"
    );
    let (status, second) = runtime_post_for_url(
        &url.replace("ws://", "http://")
            .replace("/gateway", "/gateway/ticket"),
        "bob",
        &json!({"protocol":PROTOCOL}),
    )
    .await;
    assert_eq!(status, 200, "{second}");
    let mut other = socket_connect(&url, PROTOCOL, second["credential"].as_str().unwrap()).await;
    let other_open = socket_json(&mut runtime).await;
    let other_id = other_open["connectionId"].as_str().unwrap();
    assert_ne!(other_id, connection);
    runtime
        .send(Message::Binary(
            Frame {
                direction: Direction::Runtime,
                connection_id: other_id.into(),
                payload: Payload::Binary(vec![255, 0]),
            }
            .encode()
            .unwrap(),
        ))
        .await
        .unwrap();
    assert_eq!(socket_message(&mut other).await.into_data(), vec![255, 0]);
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(75), client.next())
            .await
            .is_err(),
        "A runtime response crossed client connections"
    );
    runtime.close(None).await.unwrap();
    assert_eq!(socket_json(&mut client).await["state"], "pending");
    assert!(
        matches!(socket_message(&mut other).await,Message::Close(Some(frame)) if u16::from(frame.code)==4100)
    );
    use sha2::{Digest, Sha256};
    let legacy_bearer = "retained-runtime-bearer";
    let legacy_hash: String = Sha256::digest(legacy_bearer.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    sqlx::query("UPDATE api_cloud_workspaces SET runtime_credential_hash=$1,request_config=jsonb_set(request_config,'{runtimeCredentialExpiresAtMs}',$2) WHERE workspace_id='ws-bob'").bind(legacy_hash).bind(json!(now+900000)).execute(&pool).await.unwrap();
    let mut replacement = socket_connect(
        &url,
        zuse_protocol::cloud_gateway::LEGACY_PROTOCOL,
        legacy_bearer,
    )
    .await;
    assert_eq!(
        socket_json(&mut client).await,
        json!({"_zuseGateway":"availability","state":"available","reset":true})
    );
    // A reset connection is never silently attached to a replacement runtime.
    let mut fresh = socket_connect(&url, PROTOCOL, second["credential"].as_str().unwrap()).await;
    let fresh_open = socket_json(&mut replacement).await;
    assert_ne!(fresh_open["connectionId"], connection);
    fresh.send(Message::Binary(vec![0, 255])).await.unwrap();
    let legacy_frame = socket_json(&mut replacement).await;
    assert_eq!(
        legacy_frame,
        json!({"type":"client.frame","connectionId":fresh_open["connectionId"],"encoding":"base64","payload":"AP8="})
    );
    replacement.send(Message::Text(json!({"type":"runtime.frame","connectionId":fresh_open["connectionId"],"encoding":"base64","payload":"/wA="}).to_string())).await.unwrap();
    assert_eq!(socket_message(&mut fresh).await.into_data(), vec![255, 0]);

    sqlx::query("UPDATE zuse_rust_control_owners SET generation=3 WHERE resource_id='ws-bob'")
        .execute(&pool)
        .await
        .unwrap();
    assert!(
        matches!(socket_message(&mut replacement).await,Message::Close(Some(frame)) if u16::from(frame.code)==4101)
    );
    assert!(
        matches!(socket_message(&mut fresh).await,Message::Close(Some(frame)) if u16::from(frame.code)==4101)
    );
    let _ = client.close(None).await;
    let _ = fresh.close(None).await;
    let _ = replacement.close(None).await;
    let _ = other.close(None).await;
    stop.send(()).unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), server)
        .await
        .unwrap()
        .unwrap();
    pool.close().await;
    sqlx::raw_sql(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .unwrap();
    admin.close().await;
}
async fn runtime_post_for_url(url: &str, token: &str, body: &Value) -> (u16, Value) {
    let response = reqwest::Client::new()
        .post(url)
        .bearer_auth(token)
        .json(body)
        .send()
        .await
        .unwrap();
    (response.status().as_u16(), response.json().await.unwrap())
}

#[cfg(feature = "runtime-acceptance")]
mod runtime_engine {
    use super::*;
    use async_trait::async_trait;
    use futures::{StreamExt, stream::BoxStream};
    use std::{
        sync::atomic::{AtomicUsize, Ordering},
        time::Duration,
    };
    use tokio::sync::Notify;
    use zuse_engine::{EngineCore, HarnessRegistry};
    use zuse_protocol::{AgentEvent, DoneStatus, HarnessId, Model, ReasoningLevel, RunRequest};
    use zuse_providers::{Harness, HarnessError, RunControls};
    struct Provider {
        started: Arc<Notify>,
        release: Arc<Notify>,
        runs: Arc<AtomicUsize>,
    }
    #[async_trait]
    impl Harness for Provider {
        fn id(&self) -> HarnessId {
            HarnessId::Mock
        }
        fn display_name(&self) -> &str {
            "Cloud acceptance provider"
        }
        fn supports_steering(&self) -> bool {
            false
        }
        fn steering_mode(&self) -> zuse_protocol::SteeringMode {
            zuse_protocol::SteeringMode::TurnBoundary
        }
        fn reasoning_levels(&self) -> &[ReasoningLevel] {
            &[]
        }
        async fn models(&self) -> Result<Vec<Model>, HarnessError> {
            Ok(vec![])
        }
        async fn run(
            &self,
            request: RunRequest,
            _: RunControls,
        ) -> Result<BoxStream<'static, Result<AgentEvent, HarnessError>>, HarnessError> {
            self.runs.fetch_add(1, Ordering::SeqCst);
            self.started.notify_one();
            let release = self.release.clone();
            let cwd = std::path::PathBuf::from(request.cwd);
            Ok(futures::stream::once(async move {
                release.notified().await;
                std::fs::write(cwd.join("cloud-result.txt"), "retained change").unwrap();
                Ok(AgentEvent::TextDelta {
                    text: "Original turn completed after the client closed".into(),
                })
            })
            .chain(futures::stream::iter([Ok(AgentEvent::Done {
                status: DoneStatus::Completed,
                result: None,
                error: None,
                session_id: Some("original-agent-session".into()),
            })]))
            .boxed())
        }
    }
    fn engine(path: &std::path::Path, provider: Arc<Provider>) -> EngineCore {
        let registry = Arc::new(HarnessRegistry::new());
        registry.register(provider);
        EngineCore::assemble_with_identity(
            path,
            registry,
            HarnessId::Mock,
            None,
            "workspace-ws-bob",
            "user-bob",
        )
        .unwrap()
    }
    #[tokio::test]
    #[ignore = "requires explicit isolated localhost PostgreSQL and engine acceptance feature"]
    async fn rust_cloud_turn_survives_client_closure_and_engine_reopen_without_reexecution() {
        use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
        use ed25519_dalek::SigningKey;
        use tokio_util::sync::CancellationToken;
        use zuse_cloud::{crypto::MasterKey, runtime::RuntimeControl, tickets::TicketSigner};
        use zuse_protocol::cloud_gateway::{RUST_PROTOCOL, RUST_RPC};
        use zuse_rpc::{cloud, methods};
        let (admin, pool, schema) = fixture().await;
        let now = chrono::Utc::now().timestamp_millis();
        sqlx::query(
            "INSERT INTO zuse_rust_control_owners VALUES ('workspace','ws-bob','rust',2,$1)",
        )
        .bind(now)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "UPDATE api_cloud_workspaces SET request_config=$1 WHERE workspace_id='ws-bob'",
        )
        .bind(json!({"runtimeGeneration":2,"gatewayEpoch":5,"runtimeRpcProtocol":RUST_RPC,"rustRuntimeStorage":{"format":"zuse-rust-engine-v1","storageIncarnationId":"storage-bob"}}))
        .execute(&pool)
        .await
        .unwrap();
        let mint = SigningKey::from_bytes(&[11; 32]);
        let jwk=json!({"kty":"OKP","crv":"Ed25519","d":URL_SAFE_NO_PAD.encode([11;32]),"x":URL_SAFE_NO_PAD.encode(mint.verifying_key().as_bytes())}).to_string();
        let signer = TicketSigner::from_jwk(&jwk, "https://api-rust.zuse.sh").unwrap();
        let legacy_runtime_ticket = signer
            .runtime("user-bob", "ws-bob", 2, 5, now + 900000)
            .unwrap();
        use sha2::{Digest, Sha256};
        let boot = "rust-guest-boot";
        let boot_hash: String = Sha256::digest(boot.as_bytes())
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        sqlx::query("UPDATE api_cloud_workspaces SET runtime_boot_token_hash=$1,runtime_boot_token_expires_at=$2 WHERE workspace_id='ws-bob'").bind(boot_hash).bind(now+120000).execute(&pool).await.unwrap();
        let master = MasterKey::parse(&URL_SAFE_NO_PAD.encode([12; 32])).unwrap();
        let intent = json!({"commandId":"original-command","turnId":"original-turn","title":"Cloud conversation","agent":"mock","model":"fixture-model","permissions":[],"firstMessage":"Finish while the client is closed"});
        let ciphertext = master
            .seal(
                b"zuse-cloud-workspace-launch-intent-v1\nuser-bob\nws-bob",
                intent.to_string().as_bytes(),
            )
            .unwrap();
        sqlx::query("INSERT INTO api_cloud_workspace_launch_intents VALUES ('ws-bob','user-bob','chat-bob','session-bob','original-turn','original-command',$1,$2,$3)").bind(ciphertext).bind(now+120000).bind(now).execute(&pool).await.unwrap();
        let control = Arc::new(RuntimeControl::from_pool(pool.clone(), master, signer));
        let app = router(
            AppState {
                store: Store::from_pool(pool.clone()),
                identity: Arc::new(TestIdentity),
                organizations_enabled: false,
                organization_ids: Arc::new(vec![]),
                runtime: Some(control.clone()),
                gateway: Arc::new(zuse_cloud::gateway::Hub::default()),
            },
            vec![],
        );
        let crypto_fixture: Value =
            serde_json::from_str(include_str!("fixtures/cloud-crypto.json")).unwrap();
        let guest_key = SigningKey::from_bytes(&[13; 32]);
        let public = json!({"kty":"OKP","crv":"Ed25519","x":URL_SAFE_NO_PAD.encode(guest_key.verifying_key().as_bytes())});
        let body = json!({"credentialPublicJwk":crypto_fixture["runtimePublicJwk"].to_string(),"signingPublicJwk":public.to_string(),"rpcProtocol":RUST_RPC});
        let (status, bootstrap) = runtime_post(
            &app,
            "/v1/cloud/workspaces/ws-bob/runtime/bootstrap",
            boot,
            &body,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{bootstrap}");
        assert_eq!(bootstrap["launchIntent"], intent);
        assert_eq!(bootstrap["chatId"], "chat-bob");
        assert_eq!(bootstrap["initialSessionId"], "session-bob");
        assert_eq!(bootstrap["gatewayProtocol"], RUST_PROTOCOL);
        assert_eq!(
            runtime_post(
                &app,
                "/v1/cloud/workspaces/ws-bob/runtime/bootstrap/ack",
                bootstrap["runtimeCredential"].as_str().unwrap(),
                &json!({"runtimeGeneration":2,"gatewayEpoch":5})
            )
            .await
            .0,
            StatusCode::OK
        );
        let (status, summary) = request(&app, "/v1/cloud/workspaces/ws-bob", "bob", None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(summary["rpcProtocol"], RUST_RPC);
        let ticket_path = "/v1/cloud/workspaces/ws-bob/gateway/ticket";
        assert_eq!(
            runtime_post(&app, ticket_path, "bob", &json!({})).await.0,
            StatusCode::CONFLICT
        );
        assert!(
            control
                .gateway_authorize("ws-bob", RUST_PROTOCOL, &legacy_runtime_ticket, now)
                .await
                .is_err()
        );
        let (status, ticket) = runtime_post(
            &app,
            ticket_path,
            "bob",
            &json!({"protocol":RUST_PROTOCOL,"rpcProtocol":RUST_RPC}),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{ticket}");
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!(
            "ws://{}/v1/cloud/workspaces/ws-bob/gateway",
            listener.local_addr().unwrap()
        );
        let (stop, stopped) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = stopped.await;
                })
                .await
                .unwrap();
        });
        let temp = tempfile::tempdir().unwrap();
        let data = temp.path().join("runtime");
        let checkout = temp.path().join("checkout");
        std::fs::create_dir(&checkout).unwrap();
        let provider = Arc::new(Provider {
            started: Arc::new(Notify::new()),
            release: Arc::new(Notify::new()),
            runs: Arc::new(AtomicUsize::new(0)),
        });
        let core = engine(&data, provider.clone());
        use zuse_protocol::cloud::{CloudStorageEnrollment, CloudStoragePhase};
        let prepared = control
            .enroll_storage(
                "ws-bob",
                bootstrap["runtimeCredential"].as_str().unwrap(),
                CloudStorageEnrollment {
                    phase: CloudStoragePhase::Prepare,
                    runtime_generation: 2,
                    gateway_epoch: 5,
                    storage_incarnation_id: "storage-bob".into(),
                    enrollment_id: None,
                },
                now,
            )
            .await
            .unwrap();
        control
            .enroll_storage(
                "ws-bob",
                bootstrap["runtimeCredential"].as_str().unwrap(),
                CloudStorageEnrollment {
                    phase: CloudStoragePhase::Commit,
                    runtime_generation: 2,
                    gateway_epoch: 5,
                    storage_incarnation_id: "storage-bob".into(),
                    enrollment_id: Some(prepared["enrollmentId"].as_str().unwrap().into()),
                },
                now,
            )
            .await
            .unwrap();
        let runtime_grant = cloud::Grant {
            workspace_id: "ws-bob".into(),
            ws_url: url.clone(),
            protocol: RUST_PROTOCOL.into(),
            rpc_protocol: RUST_RPC.into(),
            credential: bootstrap["runtimeGatewayCredential"]
                .as_str()
                .unwrap()
                .into(),
            generation: 2,
            gateway_epoch: 5,
            expires_at: now + 900000,
        };
        let runtime_cancel = CancellationToken::new();
        let guest = tokio::spawn({
            let service = core.rpc_service();
            let grant = runtime_grant.clone();
            let cancel = runtime_cancel.clone();
            async move { cloud::serve_guest(service, grant, "ws-bob", "user-bob", cancel).await }
        });
        let client_grant = cloud::Grant {
            ws_url: url.clone(),
            ..serde_json::from_value(ticket.clone()).unwrap()
        };
        let client = cloud::connect(client_grant.clone(), "ws-bob", CancellationToken::new())
            .await
            .unwrap();
        client.client.call(methods::MUTATE,json!({"op":"createChat","chatId":"chat-bob","deviceId":core.device_id,"cwd":checkout,"config":{"harness":"mock","sandbox":"workspace-write"}})).await.unwrap();
        client
            .client
            .call(
                methods::CREATE_THREAD_SESSION,
                json!({"chatId":bootstrap["initialSessionId"],"threadId":bootstrap["chatId"]}),
            )
            .await
            .unwrap();
        let command = json!({"chatId":bootstrap["initialSessionId"],"entry":{"id":intent["commandId"],"payload":{"kind":"run","messageId":intent["turnId"],"request":{"prompt":intent["firstMessage"],"model":intent["model"],"harness":"mock","cwd":checkout,"sandbox":"workspace-write"}},"issuedBy":"cloud-client","issuedAt":now,"expiresAt":now+120000,"status":"pending"}});
        let accepted = client
            .client
            .call(methods::RELAY_COMMAND, command.clone())
            .await
            .unwrap();
        assert_eq!(accepted["outcome"], "executed");
        tokio::time::timeout(Duration::from_secs(10), provider.started.notified())
            .await
            .unwrap();
        client.closed.cancel();
        drop(client);
        assert!(core.sessions.has_run("session-bob"));
        provider.release.notify_one();
        tokio::time::timeout(Duration::from_secs(10), async {
            while core.sessions.has_run("session-bob") {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(checkout.join("cloud-result.txt")).unwrap(),
            "retained change"
        );
        let reopened = cloud::connect(client_grant.clone(), "ws-bob", CancellationToken::new())
            .await
            .unwrap();
        let duplicate = reopened
            .client
            .call(methods::RELAY_COMMAND, command.clone())
            .await
            .unwrap();
        assert_eq!(duplicate["outcome"], "duplicate");
        let mut history = reopened
            .client
            .subscribe(methods::WATCH_DOC_MESSAGES, json!({"chatId":"session-bob"}))
            .await
            .unwrap();
        let first = tokio::time::timeout(Duration::from_secs(5), history.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(
            first
                .to_string()
                .contains("Original turn completed after the client closed"),
            "{first}"
        );
        drop(history);
        reopened.closed.cancel();
        drop(reopened);
        runtime_cancel.cancel();
        guest.await.unwrap().unwrap();
        core.shutdown().await;
        drop(core);
        let core = engine(&data, provider.clone());
        let cancel = CancellationToken::new();
        let guest = tokio::spawn({
            let service = core.rpc_service();
            let grant = runtime_grant;
            let cancel = cancel.clone();
            async move { cloud::serve_guest(service, grant, "ws-bob", "user-bob", cancel).await }
        });
        let reopened = cloud::connect(client_grant, "ws-bob", CancellationToken::new())
            .await
            .unwrap();
        let duplicate = reopened
            .client
            .call(methods::RELAY_COMMAND, command)
            .await
            .unwrap();
        assert_eq!(duplicate["outcome"], "duplicate");
        let mut history = reopened
            .client
            .subscribe(methods::WATCH_DOC_MESSAGES, json!({"chatId":"session-bob"}))
            .await
            .unwrap();
        let first = tokio::time::timeout(Duration::from_secs(5), history.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(
            first
                .to_string()
                .contains("Original turn completed after the client closed"),
            "{first}"
        );
        assert_eq!(provider.runs.load(Ordering::SeqCst), 1);
        assert_eq!(
            core.workspace
                .chat("session-bob")
                .unwrap()
                .unwrap()
                .thread_id(),
            "chat-bob"
        );

        assert_eq!(
            core.workspace
                .chat("session-bob")
                .unwrap()
                .unwrap()
                .harness_session_id
                .as_deref(),
            Some("original-agent-session")
        );
        drop(history);
        reopened.closed.cancel();
        drop(reopened);
        cancel.cancel();
        guest.await.unwrap().unwrap();
        core.shutdown().await;
        drop(core);
        stop.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(5), server)
            .await
            .unwrap()
            .unwrap();
        pool.close().await;
        sqlx::raw_sql(&format!("DROP SCHEMA {schema} CASCADE"))
            .execute(&admin)
            .await
            .unwrap();
        admin.close().await;
    }
    struct GuestApi {
        pool: PgPool,
        admin: PgPool,
        schema: String,
        origin: String,
        control: Arc<zuse_cloud::runtime::RuntimeControl>,
        stop: tokio::sync::oneshot::Sender<()>,
        server: tokio::task::JoinHandle<()>,
    }
    impl GuestApi {
        async fn start() -> Self {
            use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD as B64};
            use ed25519_dalek::SigningKey;
            use sha2::{Digest, Sha256};
            use zuse_cloud::{crypto::MasterKey, runtime::RuntimeControl, tickets::TicketSigner};
            let (admin, pool, schema) = fixture().await;
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let origin = format!("http://{}", listener.local_addr().unwrap());
            let mint = SigningKey::from_bytes(&[14; 32]);
            let jwk = json!({"kty":"OKP","crv":"Ed25519","d":B64.encode([14;32]),"x":B64.encode(mint.verifying_key().as_bytes())});
            let signer = TicketSigner::from_jwk(&jwk.to_string(), &origin).unwrap();
            let master = MasterKey::parse(&B64.encode([15; 32])).unwrap();
            let now = chrono::Utc::now().timestamp_millis();
            let hash: String = Sha256::digest(b"packaged-guest-boot")
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect();
            sqlx::query(
                "INSERT INTO zuse_rust_control_owners VALUES ('workspace','ws-bob','rust',2,$1)",
            )
            .bind(now)
            .execute(&pool)
            .await
            .unwrap();
            sqlx::query("UPDATE api_cloud_workspaces SET runtime_boot_token_hash=$1,runtime_boot_token_expires_at=$2,request_config=$3 WHERE workspace_id='ws-bob'").bind(hash).bind(now+120000).bind(json!({"runtimeGeneration":2,"gatewayEpoch":5,"runtimeRpcProtocol":"zuse-rpc-v1","rustRuntimeStorage":{"format":"zuse-rust-engine-v1","storageIncarnationId":"packaged-storage"}})).execute(&pool).await.unwrap();
            let intent = json!({"commandId":"original-launch","turnId":"original-turn","title":"Guest launch","agent":"mock","model":"fixture","permissions":[],"runtimeMode":"approval-required","firstMessage":"Original guest prompt"});
            let ciphertext = master
                .seal(
                    b"zuse-cloud-workspace-launch-intent-v1\nuser-bob\nws-bob",
                    intent.to_string().as_bytes(),
                )
                .unwrap();
            sqlx::query("INSERT INTO api_cloud_workspace_launch_intents VALUES ('ws-bob','user-bob','chat-bob','session-bob','original-turn','original-launch',$1,$2,$3)").bind(ciphertext).bind(now+120000).bind(now).execute(&pool).await.unwrap();
            let control = Arc::new(RuntimeControl::from_pool(pool.clone(), master, signer));
            let app = router(
                AppState {
                    store: Store::from_pool(pool.clone()),
                    identity: Arc::new(TestIdentity),
                    organizations_enabled: false,
                    organization_ids: Arc::new(vec![]),
                    runtime: Some(control.clone()),
                    gateway: Arc::new(zuse_cloud::gateway::Hub::default()),
                },
                vec![],
            );
            let (stop, stopped) = tokio::sync::oneshot::channel();
            let server = tokio::spawn(async move {
                axum::serve(listener, app)
                    .with_graceful_shutdown(async {
                        let _ = stopped.await;
                    })
                    .await
                    .unwrap();
            });
            Self {
                pool,
                admin,
                schema,
                origin,
                control,
                stop,
                server,
            }
        }
        async fn finish(self) {
            let _ = self.stop.send(());
            tokio::time::timeout(Duration::from_secs(5), self.server)
                .await
                .unwrap()
                .unwrap();
            self.pool.close().await;
            sqlx::raw_sql(&format!("DROP SCHEMA {} CASCADE", self.schema))
                .execute(&self.admin)
                .await
                .unwrap();
            self.admin.close().await;
        }
        async fn client(&self) -> zuse_rpc::cloud::Connection {
            let (status, ticket) = runtime_post_for_url(
                &format!("{}/v1/cloud/workspaces/ws-bob/gateway/ticket", self.origin),
                "bob",
                &json!({"protocol":"zuse-workspace-rust-v1","rpcProtocol":"zuse-rpc-v1"}),
            )
            .await;
            assert_eq!(status, 200, "{ticket}");
            zuse_rpc::cloud::connect(
                serde_json::from_value(ticket).unwrap(),
                "ws-bob",
                tokio_util::sync::CancellationToken::new(),
            )
            .await
            .unwrap()
        }
    }
    #[tokio::test]
    #[ignore = "requires isolated localhost PostgreSQL and runtime acceptance feature"]
    async fn rust_cloud_guest_recovers_lost_renewal_response_and_acknowledgement_with_same_keys() {
        use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD as B64};
        use zuse_cloud_runtime::{Authority, DataLock, Identity, IdentityBinding};
        let api = GuestApi::start().await;
        let directory = tempfile::tempdir().unwrap();
        let _lock = DataLock::acquire(directory.path(), None).unwrap();
        let binding = IdentityBinding {
            workspace: "ws-bob".into(),
            api_url: api.origin.clone(),
            incarnation: "packaged-storage".into(),
            generation: 2,
        };
        let authority = Authority::new(&api.origin).unwrap();
        let mut identity =
            Identity::open(directory.path(), &binding, Some("packaged-guest-boot")).unwrap();
        let boot = authority
            .enroll(&mut identity, &binding, Some("packaged-guest-boot"))
            .await
            .unwrap();
        let key = identity
            .open_transcript_key(&boot.sealed_transcript_key)
            .unwrap();
        let wrapped: String = sqlx::query_scalar(
            "SELECT wrapped_transcript_key FROM api_cloud_workspaces WHERE workspace_id='ws-bob'",
        )
        .fetch_one(&api.pool)
        .await
        .unwrap();
        let master = zuse_cloud::crypto::MasterKey::parse(&B64.encode([15; 32])).unwrap();
        assert_eq!(
            key.as_str(),
            master
                .open_transcript_key("user-bob", "ws-bob", &wrapped)
                .unwrap()
                .as_str()
        );
        let original_keys = identity.request();
        let request = identity.begin_renewal().unwrap();
        let now = chrono::Utc::now().timestamp_millis();
        sqlx::query("UPDATE api_cloud_workspaces SET request_config=jsonb_set(request_config,'{runtimeCredentialExpiresAtMs}',$1) WHERE workspace_id='ws-bob'").bind(json!(now-1000)).execute(&api.pool).await.unwrap();
        let accepted = api
            .control
            .renew(
                "ws-bob",
                &boot.runtime_credential,
                zuse_cloud::runtime::RenewRequest {
                    request_id: request.clone(),
                    proof: identity.renewal_proof(&request, now).unwrap(),
                },
                now,
            )
            .await
            .unwrap();
        // The network lost the committed response and the local ACK marker.
        drop(identity);
        let path = directory.path().join("cloud-runtime-identity.json");
        let mut stored: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        stored["bootstrapAcknowledged"] = json!(false);
        std::fs::write(&path, serde_json::to_vec(&stored).unwrap()).unwrap();
        let mut recovered = Identity::open(directory.path(), &binding, None).unwrap();
        let renewed = authority
            .enroll(&mut recovered, &binding, None)
            .await
            .unwrap();
        assert_eq!(renewed.runtime_credential, accepted["runtimeCredential"]);
        assert!(recovered.acknowledged());
        assert!(!recovered.has_pending_renewal());
        assert_eq!(
            original_keys.credential_public_jwk,
            recovered.request().credential_public_jwk
        );
        assert_eq!(
            original_keys.signing_public_jwk,
            recovered.request().signing_public_jwk
        );
        assert_eq!(
            key.as_str(),
            recovered
                .open_transcript_key(&renewed.sealed_transcript_key)
                .unwrap()
                .as_str()
        );
        sqlx::query(
            "UPDATE zuse_rust_control_owners SET backend='legacy' WHERE resource_id='ws-bob'",
        )
        .execute(&api.pool)
        .await
        .unwrap();
        let error = authority.renew(&mut recovered).await.unwrap_err();
        assert!(!zuse_cloud_runtime::supervisor::retryable(&error));
        api.finish().await;
    }
    fn guest_command(
        api: &GuestApi,
        directory: &std::path::Path,
        checkout: &std::path::Path,
        boot: Option<&std::path::Path>,
        log: &std::path::Path,
    ) -> tokio::process::Command {
        let binary = std::env::var_os("ZUSE_CLOUD_TEST_GUEST_BIN")
            .expect("Set ZUSE_CLOUD_TEST_GUEST_BIN to the built zuse-cloud-runtime executable");
        let mut command = tokio::process::Command::new(binary);
        command
            .args([
                "--workspace-id",
                "ws-bob",
                "--api-url",
                &api.origin,
                "--storage-incarnation-id",
                "packaged-storage",
                "--generation",
                "2",
                "--ipc-port",
                "0",
                "--test-harness",
            ])
            .arg("--data-dir")
            .arg(directory)
            .arg("--workspace-root")
            .arg(checkout)
            .env("RUST_LOG", "warn")
            .env_remove("ZUSE_RUNTIME_BOOT_TOKEN")
            .env_remove("ZUSE_WORKSPACE_RUNTIME_LOCK_FD")
            .env_remove("ZUSE_RUNTIME_BOOT_TOKEN_FILE")
            .kill_on_drop(true)
            .stdout(std::process::Stdio::null())
            .stderr(std::fs::File::create(log).unwrap());
        if let Some(boot) = boot {
            command.arg("--boot-token-file").arg(boot);
        }
        command
    }
    async fn stop_guest(child: &mut tokio::process::Child) {
        #[cfg(unix)]
        unsafe {
            libc::kill(child.id().unwrap() as i32, libc::SIGTERM);
        }
        #[cfg(not(unix))]
        child.start_kill().unwrap();
        let status = tokio::time::timeout(Duration::from_secs(10), child.wait())
            .await
            .unwrap()
            .unwrap();
        assert!(status.success());
    }
    #[tokio::test]
    #[ignore = "requires isolated PostgreSQL and ZUSE_CLOUD_TEST_GUEST_BIN"]
    async fn rust_cloud_guest_executable_enrolls_adopts_launch_and_reopens_original_storage() {
        let api = GuestApi::start().await;
        let temporary = tempfile::tempdir().unwrap();
        let directory = temporary.path().join("runtime");
        let checkout = temporary.path().join("checkout");
        std::fs::create_dir(&checkout).unwrap();
        let boot = temporary.path().join("boot-token");
        std::fs::write(&boot, "packaged-guest-boot").unwrap();
        let log = temporary.path().join("guest.log");
        let mut child = guest_command(&api, &directory, &checkout, Some(&boot), &log)
            .spawn()
            .unwrap();
        tokio::time::timeout(Duration::from_secs(20), async {
            loop {
                if let Some(status) = child.try_wait().unwrap() {
                    panic!(
                        "Guest exited {status}: {}",
                        std::fs::read_to_string(&log).unwrap()
                    );
                }
                let state: String = sqlx::query_scalar(
                    "SELECT state FROM api_cloud_workspaces WHERE workspace_id='ws-bob'",
                )
                .fetch_one(&api.pool)
                .await
                .unwrap();
                if state == "ready" {
                    let count: i64 = sqlx::query_scalar(
                        "SELECT count(*) FROM api_cloud_workspace_launch_intents",
                    )
                    .fetch_one(&api.pool)
                    .await
                    .unwrap();
                    if count == 0 {
                        break;
                    }
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        })
        .await
        .unwrap();
        let client = api.client().await;
        let first = client
            .client
            .call(zuse_rpc::methods::ENGINE_INFO, json!({}))
            .await
            .unwrap();
        let mut messages = client
            .client
            .subscribe(
                zuse_rpc::methods::WATCH_DOC_MESSAGES,
                json!({"chatId":"session-bob"}),
            )
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let snapshot = messages.recv().await.unwrap();
                if snapshot.to_string().contains("Cloud runtime fixture reply") {
                    break;
                }
            }
        })
        .await
        .unwrap();
        drop(messages);
        client.closed.cancel();
        drop(client);
        stop_guest(&mut child).await;
        let stored: Value = serde_json::from_slice(
            &std::fs::read(directory.join("cloud-runtime-identity.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(stored["accountId"], "user-bob");
        assert_eq!(stored["storageIncarnationId"], "packaged-storage");
        assert_eq!(stored["bootstrapAcknowledged"], true);
        let mut restarted = guest_command(&api, &directory, &checkout, None, &log)
            .spawn()
            .unwrap();
        // Cached ready state is insufficient: a fresh gateway client must reach
        // this guest and its retained database after the previous socket retires.
        let client = api.client().await;
        let next = client
            .client
            .call(zuse_rpc::methods::ENGINE_INFO, json!({}))
            .await
            .unwrap();
        assert_eq!(first["deviceId"], next["deviceId"]);
        let mut messages = client
            .client
            .subscribe(
                zuse_rpc::methods::WATCH_DOC_MESSAGES,
                json!({"chatId":"session-bob"}),
            )
            .await
            .unwrap();
        let snapshot = tokio::time::timeout(Duration::from_secs(5), messages.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(snapshot.to_string().contains("Original guest prompt"));
        assert!(snapshot.to_string().contains("Cloud runtime fixture reply"));
        assert_eq!(
            snapshot
                .to_string()
                .matches("Cloud runtime fixture reply")
                .count(),
            1,
            "Restart re-executed the original command"
        );

        drop(messages);
        client.closed.cancel();
        drop(client);
        stop_guest(&mut restarted).await;
        let after: Value = serde_json::from_slice(
            &std::fs::read(directory.join("cloud-runtime-identity.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(stored["credentialPublicJwk"], after["credentialPublicJwk"]);
        assert_eq!(stored["signingPublicJwk"], after["signingPublicJwk"]);
        let database = directory.join("orgs/ws-bob/user-bob/docs.sqlite3");
        assert!(database.is_file());
        let saved = database.with_extension("saved");
        std::fs::rename(&database, &saved).unwrap();
        let status = tokio::time::timeout(
            Duration::from_secs(10),
            guest_command(&api, &directory, &checkout, None, &log).status(),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(!status.success());
        assert!(!database.exists());
        assert!(saved.is_file());
        assert!(
            std::fs::read_to_string(&log)
                .unwrap()
                .contains("runtime_storage_missing")
        );
        // Lose both local marker and database, while the persisted control-plane
        // barrier still records that this workspace has executed accepted work.
        let empty = temporary.path().join("empty-runtime");
        std::fs::create_dir(&empty).unwrap();
        std::fs::copy(
            directory.join("cloud-runtime-identity.json"),
            empty.join("cloud-runtime-identity.json"),
        )
        .unwrap();
        let status = tokio::time::timeout(
            Duration::from_secs(10),
            guest_command(&api, &empty, &checkout, None, &log).status(),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(!status.success());
        assert!(!empty.join("orgs").exists());
        assert!(saved.is_file());
        assert!(
            std::fs::read_to_string(&log)
                .unwrap()
                .contains("runtime_storage_recovery_required")
        );
        let credential = after["bootstrap"]["runtimeCredential"].as_str().unwrap();
        let url = format!("{}/v1/cloud/workspaces/ws-bob/ready", api.origin);
        assert_eq!(runtime_post_for_url(&url,credential,&json!({"phase":"agent-started","launchCommandId":"unrelated-command","sessionHeadVersion":0})).await.0,409);
        sqlx::query("UPDATE api_cloud_workspaces SET state='archived',desired_state='archived' WHERE workspace_id='ws-bob'").execute(&api.pool).await.unwrap();
        assert_eq!(runtime_post_for_url(&url,credential,&json!({"phase":"agent-started","launchCommandId":"original-launch","sessionHeadVersion":0})).await.0,401);
        api.finish().await;
    }
}
