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
        let token = if std::env::var_os("ZUSE_CLOUD_MODEL_ACCEPTANCE").is_some()
            && token.starts_with("fixture.")
            && token.ends_with(".signature")
        {
            use base64::Engine as _;
            let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(token.split('.').nth(1).unwrap())
                .unwrap();
            let claims: Value = serde_json::from_slice(&payload).unwrap();
            assert_eq!(claims["sub"], "user-bob");
            "bob"
        } else {
            token
        };
        let token = if token == "dev-user"
            && std::env::var_os("ZUSE_CLOUD_BILLING_UI_FIXTURE_FILE").is_some()
        {
            "alice"
        } else {
            token
        };
        let token =
            if token == "dev-user" && std::env::var_os("ZUSE_CLOUD_UI_FIXTURE_FILE").is_some() {
                "bob"
            } else {
                token
            };
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
    sqlx::raw_sql(zuse_cloud::runtime::mailbox::MIGRATION)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::raw_sql(zuse_cloud::runtime::allocation::MIGRATION)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::raw_sql(zuse_cloud::runtime::lifecycle::MIGRATION)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::raw_sql(include_str!("../migrations/0012_provider_receipts.sql"))
        .execute(&pool)
        .await
        .unwrap();
    sqlx::raw_sql(zuse_cloud::runtime::recovery::MIGRATION)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::raw_sql(include_str!("../migrations/0013_recovery_receipts.sql"))
        .execute(&pool)
        .await
        .unwrap();
    sqlx::raw_sql(zuse_cloud::runtime::restore::MIGRATION)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::raw_sql(include_str!("../migrations/0014_restore_receipts.sql"))
        .execute(&pool)
        .await
        .unwrap();
    sqlx::raw_sql(zuse_cloud::runtime::task_history::MIGRATION)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::raw_sql(zuse_cloud::runtime::deletion::MIGRATION)
        .execute(&pool)
        .await
        .unwrap();
    // Provider grants and credential delivery journal their receipts.
    sqlx::raw_sql(include_str!(
        "../migrations/0024_credential_delivery_journal.sql"
    ))
    .execute(&pool)
    .await
    .unwrap();
    (admin, pool, schema)
}

#[tokio::test]
#[ignore = "requires explicit isolated localhost PostgreSQL"]
async fn personal_computer_discovery_is_account_scoped_and_excludes_host_secrets() {
    let (admin, pool, schema) = fixture().await;
    sqlx::raw_sql("CREATE TABLE api_dpop_proofs (thumbprint text NOT NULL, jti text NOT NULL, issued_at bigint NOT NULL, expires_at bigint NOT NULL, PRIMARY KEY(thumbprint,jti)); CREATE TABLE api_revoked_dpop_keys (thumbprint text PRIMARY KEY, revoked_at bigint NOT NULL);")
        .execute(&pool).await.unwrap();
    let store = Store::from_pool(pool.clone());
    sqlx::raw_sql("CREATE TABLE api_devices(device_id text PRIMARY KEY,account_id text NOT NULL,platform text NOT NULL,push_token text,dpop_jwk jsonb,dpop_thumbprint text,updated_at bigint NOT NULL)").execute(&pool).await.unwrap();
    let key = json!({"kty":"EC"});
    store
        .register_native_device("phone", "alice", "android", None, &key, "key-a", 1)
        .await
        .unwrap();
    store
        .register_native_device("phone", "alice", "android", Some("push"), &key, "key-a", 2)
        .await
        .unwrap();
    assert!(
        store
            .register_native_device("phone", "bob", "android", None, &key, "key-a", 3)
            .await
            .is_err()
    );
    assert!(
        store
            .register_native_device("phone", "alice", "android", None, &key, "key-b", 3)
            .await
            .is_err()
    );
    let thumbprint = "a".repeat(43);
    let now = 1_000_000;
    let (first, duplicate) = tokio::join!(
        store.consume_device_proof(&thumbprint, "same-proof", now, now),
        store.consume_device_proof(&thumbprint, "same-proof", now, now),
    );
    assert_ne!(
        first.is_ok(),
        duplicate.is_ok(),
        "Exactly one concurrent proof must succeed"
    );
    assert!(
        store
            .consume_device_proof(&thumbprint, "stale", now - 121_000, now)
            .await
            .is_err()
    );
    assert!(
        store
            .consume_device_proof(&thumbprint, "future", now + 31_000, now)
            .await
            .is_err()
    );
    sqlx::query("INSERT INTO api_revoked_dpop_keys VALUES ($1,$2)")
        .bind(&thumbprint)
        .bind(now)
        .execute(&pool)
        .await
        .unwrap();
    assert!(
        store
            .consume_device_proof(&thumbprint, "new-proof", now, now)
            .await
            .is_err()
    );
    sqlx::raw_sql("CREATE TABLE api_environments (environment_id text PRIMARY KEY, account_id text NOT NULL, org_id text, provider_kind text NOT NULL, label text, wire_protocol_version bigint, capabilities jsonb, linked_at bigint NOT NULL, environment_public_key text, http_base_url text, ws_base_url text);
        INSERT INTO api_environments VALUES
        ('alice-host','user-alice',NULL,'desktop','Alice computer',1,'{\"features\":[\"zuse-rpc-json-v1\"]}',3,'PRIVATE_KEY','PRIVATE_HTTP','PRIVATE_WS'),
        ('bob-host','user-bob',NULL,'desktop','Bob computer',1,'{}',2,'PRIVATE_KEY','PRIVATE_HTTP','PRIVATE_WS'),
        ('org-host','user-alice','org-team','desktop','Organization computer',1,'{}',1,'PRIVATE_KEY','PRIVATE_HTTP','PRIVATE_WS');")
        .execute(&pool).await.unwrap();
    {
        use sha2::{Digest, Sha256};
        sqlx::raw_sql("ALTER TABLE api_environments ADD COLUMN last_seen_at bigint, ADD COLUMN runtime_version text, ADD COLUMN service_state text; CREATE TABLE api_environment_credentials(environment_id text NOT NULL,account_id text NOT NULL,credential_hash text NOT NULL,revoked_at bigint);").execute(&pool).await.unwrap();
        let credential = "zenv_test-heartbeat-secret";
        let hash = format!("{:x}", Sha256::digest(credential.as_bytes()));
        sqlx::query(
            "INSERT INTO api_environment_credentials VALUES ('alice-host','user-alice',$1,NULL)",
        )
        .bind(hash)
        .execute(&pool)
        .await
        .unwrap();
        let heartbeat = json!({"runtimeVersion":"0.25.1","wireProtocolVersion":1,"capabilities":{"features":["zuse-rpc-json-v1"]},"serviceState":"healthy","origin":{"localHttpHost":"127.0.0.1","localHttpPort":28655}});
        store
            .heartbeat_native_environment("alice-host", credential, &heartbeat, 100)
            .await
            .unwrap();
        assert!(
            store
                .heartbeat_native_environment("bob-host", credential, &heartbeat, 200)
                .await
                .is_err()
        );
        assert!(
            store
                .heartbeat_native_environment("alice-host", "zenv_wrong", &heartbeat, 200)
                .await
                .is_err()
        );
        sqlx::query("UPDATE api_environment_credentials SET revoked_at=101")
            .execute(&pool)
            .await
            .unwrap();
        assert!(
            store
                .heartbeat_native_environment("alice-host", credential, &heartbeat, 200)
                .await
                .is_err()
        );
        let last_seen: i64 = sqlx::query_scalar(
            "SELECT last_seen_at FROM api_environments WHERE environment_id='alice-host'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(last_seen, 100);
    }
    let app = router(
        AppState {
            store: Store::from_pool(pool.clone()),
            identity: Arc::new(TestIdentity),
            organizations_enabled: true,
            organization_ids: Arc::new(vec!["org-team".into()]),
            runtime: None,
            gateway: Arc::new(zuse_cloud::gateway::Hub::default()),
        },
        vec![],
    );
    assert_eq!(
        request(&app, "/v1/environments", "", None).await.0,
        StatusCode::UNAUTHORIZED
    );
    let (status, body) = request(&app, "/v1/environments", "alice", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["environments"].as_array().unwrap().len(), 1);
    assert_eq!(body["environments"][0]["environmentId"], "alice-host");
    assert_eq!(body["environments"][0]["wireProtocolVersion"], 1);
    assert_eq!(
        body["environments"][0]["capabilities"]["features"][0],
        "zuse-rpc-json-v1"
    );
    for secret in [
        "PRIVATE",
        "bob-host",
        "org-host",
        "account_id",
        "publicKey",
        "httpBaseUrl",
        "wsBaseUrl",
    ] {
        assert!(!body.to_string().contains(secret));
    }
    let (_, bob) = request(&app, "/v1/environments", "bob", None).await;
    {
        use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
        use p256::{ecdsa::SigningKey, pkcs8::EncodePrivateKey};
        let seed = [9u8; 32];
        let mint_key = ed25519_dalek::SigningKey::from_bytes(&seed);
        sqlx::raw_sql("CREATE TABLE api_link_challenges(challenge_id text PRIMARY KEY,account_id text NOT NULL,challenge text NOT NULL,relay_issuer text NOT NULL,expires_at bigint NOT NULL,consumed_at bigint)").execute(&pool).await.unwrap();
        let issuer = "http://127.0.0.1:28700";
        let challenge = store
            .create_native_link_challenge("user-alice", issuer, 1_000_000)
            .await
            .unwrap();
        let public = json!({"kty":"OKP","crv":"Ed25519","x":URL_SAFE_NO_PAD.encode(mint_key.verifying_key().as_bytes())});
        let der = mint_key.to_pkcs8_der().unwrap();
        let encoding = jsonwebtoken::EncodingKey::from_ed_der(der.as_bytes());
        let mut header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::EdDSA);
        header.typ = Some("environment-link-proof+jwt".into());
        let link_proof=jsonwebtoken::encode(&header,&json!({"aud":issuer,"challenge":challenge["challenge"],"environmentId":"desktop","iat":1000,"exp":1300}),&encoding).unwrap();
        let id = challenge["challengeId"].as_str().unwrap();
        assert!(
            store
                .consume_native_link_proof(
                    "user-bob",
                    id,
                    "desktop",
                    &public,
                    &link_proof,
                    issuer,
                    1_000_000
                )
                .await
                .is_err()
        );
        assert!(
            store
                .consume_native_link_proof(
                    "user-alice",
                    id,
                    "other-host",
                    &public,
                    &link_proof,
                    issuer,
                    1_000_000
                )
                .await
                .is_err()
        );
        let (a, b) = tokio::join!(
            store.consume_native_link_proof(
                "user-alice",
                id,
                "desktop",
                &public,
                &link_proof,
                issuer,
                1_000_000
            ),
            store.consume_native_link_proof(
                "user-alice",
                id,
                "desktop",
                &public,
                &link_proof,
                issuer,
                1_000_000
            )
        );
        assert_ne!(a.is_ok(), b.is_ok());
        let expired = store
            .create_native_link_challenge("user-alice", issuer, 0)
            .await
            .unwrap();
        assert!(
            store
                .consume_native_link_proof(
                    "user-alice",
                    expired["challengeId"].as_str().unwrap(),
                    "desktop",
                    &public,
                    &link_proof,
                    issuer,
                    1_000_000
                )
                .await
                .is_err()
        );
        sqlx::raw_sql("ALTER TABLE api_environment_credentials ADD COLUMN credential_id text, ADD COLUMN created_at bigint;").execute(&pool).await.unwrap();
        let finish_challenge = store
            .create_native_link_challenge("user-alice", issuer, 1_000_000)
            .await
            .unwrap();
        let finish_id = finish_challenge["challengeId"].as_str().unwrap();
        let finish_proof=jsonwebtoken::encode(&header,&json!({"aud":issuer,"challenge":finish_challenge["challenge"],"environmentId":"linked-host","iat":1000,"exp":1300}),&encoding).unwrap();
        sqlx::query("ALTER TABLE api_environment_credentials ADD CONSTRAINT reject_test_insert CHECK (false) NOT VALID").execute(&pool).await.unwrap();
        assert!(
            store
                .finish_native_host_link(
                    "user-alice",
                    finish_id,
                    "linked-host",
                    &public,
                    &finish_proof,
                    issuer,
                    "Desktop",
                    "https://linked.example",
                    "wss://linked.example",
                    1_000_000
                )
                .await
                .is_err()
        );
        let absent: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM api_environments WHERE environment_id='linked-host'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(absent, 0);
        let unused: bool = sqlx::query_scalar(
            "SELECT consumed_at IS NULL FROM api_link_challenges WHERE challenge_id=$1",
        )
        .bind(finish_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(unused);
        sqlx::query("ALTER TABLE api_environment_credentials DROP CONSTRAINT reject_test_insert")
            .execute(&pool)
            .await
            .unwrap();
        let credential = store
            .finish_native_host_link(
                "user-alice",
                finish_id,
                "linked-host",
                &public,
                &finish_proof,
                issuer,
                "Desktop",
                "https://linked.example",
                "wss://linked.example",
                1_000_000,
            )
            .await
            .unwrap();
        assert!(credential.starts_with("zenv_"));
        assert!(
            store
                .finish_native_host_link(
                    "user-alice",
                    finish_id,
                    "linked-host",
                    &public,
                    &finish_proof,
                    issuer,
                    "Desktop",
                    "https://linked.example",
                    "wss://linked.example",
                    1_000_000
                )
                .await
                .is_err()
        );
        let rotation = store
            .create_native_link_challenge("user-alice", issuer, 1_001_000)
            .await
            .unwrap();
        let rotation_proof=jsonwebtoken::encode(&header,&json!({"aud":issuer,"challenge":rotation["challenge"],"environmentId":"linked-host","iat":1001,"exp":1301}),&encoding).unwrap();
        sqlx::query("ALTER TABLE api_environments ADD COLUMN tunnel_hostname text, ADD COLUMN tunnel_id text, ADD COLUMN dns_record_id text, ADD COLUMN tunnel_status text").execute(&pool).await.unwrap();
        let allocation = zuse_cloud::tunnels::Allocation {
            hostname: "linked.example".into(),
            tunnel_id: "test-tunnel".into(),
            dns_record_id: "test-dns".into(),
            connector_token: zeroize::Zeroizing::new("test-connector".into()),
        };
        let next = store
            .finish_native_host_link_with_tunnel(
                "user-alice",
                rotation["challengeId"].as_str().unwrap(),
                "linked-host",
                &public,
                &rotation_proof,
                issuer,
                "Desktop",
                "https://linked.example",
                "wss://linked.example",
                1_001_000,
                Some(&allocation),
            )
            .await
            .unwrap();
        assert_ne!(next, credential);
        let saved:Value=sqlx::query_scalar("SELECT jsonb_build_object('hostname',tunnel_hostname,'tunnel',tunnel_id,'dns',dns_record_id,'status',tunnel_status) FROM api_environments WHERE environment_id='linked-host'").fetch_one(&pool).await.unwrap();
        assert_eq!(
            saved,
            json!({"hostname":"linked.example","tunnel":"test-tunnel","dns":"test-dns","status":"ready"})
        );

        let active:i64=sqlx::query_scalar("SELECT count(*) FROM api_environment_credentials WHERE environment_id='linked-host' AND revoked_at IS NULL").fetch_one(&pool).await.unwrap();
        assert_eq!(active, 1);
        let stored_plain:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM api_environment_credentials WHERE credential_hash=$1 OR credential_hash=$2)").bind(&credential).bind(&next).fetch_one(&pool).await.unwrap();
        assert!(!stored_plain);
        let mint_jwk = json!({"kty":"OKP","crv":"Ed25519","d":URL_SAFE_NO_PAD.encode(seed),"x":URL_SAFE_NO_PAD.encode(mint_key.verifying_key().as_bytes())});
        let signer = zuse_cloud::tickets::TicketSigner::from_jwk(
            &mint_jwk.to_string(),
            "http://127.0.0.1:28700",
        )
        .unwrap();
        let mut store = Store::from_pool(pool.clone());
        store.devices = Some(Arc::new(zuse_cloud::devices::Authority::new(
            signer,
            pool.clone(),
        )));
        let routes = router(
            AppState {
                store,
                identity: Arc::new(TestIdentity),
                organizations_enabled: false,
                organization_ids: Arc::new(vec![]),
                runtime: None,
                gateway: Arc::new(zuse_cloud::gateway::Hub::default()),
            },
            vec![],
        );
        let key = SigningKey::from_slice(&[8; 32]).unwrap();
        let point = key.verifying_key().to_encoded_point(false);
        let jwk = json!({"kty":"EC","crv":"P-256","x":URL_SAFE_NO_PAD.encode(point.x().unwrap()),"y":URL_SAFE_NO_PAD.encode(point.y().unwrap())});
        let der = key.to_pkcs8_der().unwrap();
        let encoding = jsonwebtoken::EncodingKey::from_ec_der(der.as_bytes());
        let proof = |path: &str, access: Option<&str>| {
            use sha2::{Digest, Sha256};
            let mut header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::ES256);
            header.typ = Some("dpop+jwt".into());
            header.jwk = Some(serde_json::from_value(jwk.clone()).unwrap());
            let mut claims = json!({"htm":"POST","htu":format!("http://127.0.0.1:28700{path}"),"iat":chrono::Utc::now().timestamp(),"jti":Uuid::new_v4().to_string()});
            if let Some(access) = access {
                claims["ath"] = json!(URL_SAFE_NO_PAD.encode(Sha256::digest(access.as_bytes())));
            }
            jsonwebtoken::encode(&header, &claims, &encoding).unwrap()
        };
        async fn post(
            app: &Router,
            path: &str,
            auth: &str,
            proof: &str,
            body: Value,
        ) -> (StatusCode, Value) {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri(path)
                        .header("Authorization", auth)
                        .header("DPoP", proof)
                        .header("Content-Type", "application/json")
                        .body(Body::from(body.to_string()))
                        .unwrap(),
                )
                .await
                .unwrap();
            let status = response.status();
            let bytes = to_bytes(response.into_body(), 65536).await.unwrap();
            (status, serde_json::from_slice(&bytes).unwrap())
        }
        let mint_path = "/v1/client/dpop-token";
        let mint_proof = proof(mint_path, None);
        assert_eq!(
            post(&routes, mint_path, "Bearer invalid", &mint_proof, json!({}))
                .await
                .0,
            StatusCode::UNAUTHORIZED
        );
        let (status, minted) =
            post(&routes, mint_path, "Bearer alice", &mint_proof, json!({})).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            post(&routes, mint_path, "Bearer alice", &mint_proof, json!({}))
                .await
                .0,
            StatusCode::UNAUTHORIZED
        );
        let token = minted["accessToken"].as_str().unwrap();
        let path = "/v1/mobile/devices";
        let registration = proof(path, Some(token));
        let body = json!({"deviceId":"native-phone","platform":"android","dpopJwk":jwk});
        let auth = format!("DPoP {token}");
        assert_eq!(
            post(&routes, path, &auth, &registration, body.clone())
                .await
                .0,
            StatusCode::OK
        );
        assert_eq!(
            post(&routes, path, &auth, &registration, body).await.0,
            StatusCode::UNAUTHORIZED
        );
        sqlx::query("UPDATE api_environments SET http_base_url='https://host.example',ws_base_url='wss://host.example',service_state='healthy',last_seen_at=(extract(epoch from clock_timestamp())*1000)::bigint WHERE environment_id='alice-host'").execute(&pool).await.unwrap();
        let connect_path = "/v1/environments/alice-host/connect";
        let request_body = json!({"requireManaged":true,"wireProtocolVersion":1});
        let (status, grant) = post(
            &routes,
            connect_path,
            &auth,
            &proof(connect_path, Some(token)),
            request_body.clone(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let connection = grant["connectToken"].as_str().unwrap();
        assert_eq!(
            jsonwebtoken::decode_header(connection)
                .unwrap()
                .typ
                .as_deref(),
            Some("connect+jwt")
        );
        let public = json!({"kty":"OKP","crv":"Ed25519","x":mint_jwk["x"]});
        let decoding =
            jsonwebtoken::DecodingKey::from_jwk(&serde_json::from_value(public).unwrap()).unwrap();
        let mut validation = jsonwebtoken::Validation::new(jsonwebtoken::Algorithm::EdDSA);
        validation.set_issuer(&["http://127.0.0.1:28700"]);
        validation.set_audience(&["zuse-env:alice-host"]);
        validation.leeway = 0;
        let claims = jsonwebtoken::decode::<Value>(connection, &decoding, &validation)
            .unwrap()
            .claims;
        assert_eq!(claims["sub"], "user-alice");
        assert_eq!(claims["environmentId"], "alice-host");
        assert_eq!(
            claims["exp"].as_i64().unwrap() - claims["iat"].as_i64().unwrap(),
            60
        );
        assert_eq!(
            grant["expiresAt"].as_i64().unwrap(),
            claims["exp"].as_i64().unwrap() * 1000
        );
        let other = "/v1/environments/bob-host/connect";
        sqlx::query("UPDATE api_environments SET last_seen_at=1 WHERE environment_id='alice-host'")
            .execute(&pool)
            .await
            .unwrap();
        let (status, offline) = post(
            &routes,
            connect_path,
            &auth,
            &proof(connect_path, Some(token)),
            request_body.clone(),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(offline["error"], "environment_offline");
        sqlx::query("UPDATE api_environments SET last_seen_at=(extract(epoch from clock_timestamp())*1000)::bigint,service_state='starting' WHERE environment_id='alice-host'").execute(&pool).await.unwrap();
        assert_eq!(
            post(
                &routes,
                connect_path,
                &auth,
                &proof(connect_path, Some(token)),
                request_body.clone()
            )
            .await
            .0,
            StatusCode::CONFLICT
        );
        sqlx::query(
            "UPDATE api_environments SET service_state='healthy' WHERE environment_id='alice-host'",
        )
        .execute(&pool)
        .await
        .unwrap();
        assert_eq!(
            post(
                &routes,
                other,
                &auth,
                &proof(other, Some(token)),
                request_body.clone()
            )
            .await
            .0,
            StatusCode::NOT_FOUND
        );
        sqlx::query("UPDATE api_environments SET ws_base_url='ws://host.example' WHERE environment_id='alice-host'").execute(&pool).await.unwrap();
        assert_eq!(
            post(
                &routes,
                connect_path,
                &auth,
                &proof(connect_path, Some(token)),
                request_body
            )
            .await
            .0,
            StatusCode::SERVICE_UNAVAILABLE
        );
    }
    assert_eq!(bob["environments"][0]["environmentId"], "bob-host");
    assert_eq!(
        request(
            &app,
            "/v1/environments",
            "admin",
            Some("organization:org-team")
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    pool.close().await;
    sqlx::raw_sql(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .unwrap();
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
    mod memory_acceptance {
        use super::*;
        include!("cases/memory.rs");
    }
    mod recovery_acceptance {
        use super::*;
        include!("cases/recovery.rs");
        include!("cases/deletion.rs");
    }
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
        requests: Arc<std::sync::Mutex<Vec<RunRequest>>>,
        supports_steering: bool,
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
            self.supports_steering
        }
        fn steering_mode(&self) -> zuse_protocol::SteeringMode {
            zuse_protocol::SteeringMode::TurnBoundary
        }
        fn reasoning_levels(&self) -> &[ReasoningLevel] {
            &[ReasoningLevel::High]
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
            self.requests.lock().unwrap().push(request.clone());
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
            requests: Arc::new(std::sync::Mutex::new(Vec::new())),
            supports_steering: false,
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
                    capabilities: None,
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
                    capabilities: None,
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
            storage_binding: None,
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
    fn queued_cloud_engine(
        path: &std::path::Path,
        provider: Arc<Provider>,
        durable: bool,
    ) -> EngineCore {
        let registry = Arc::new(HarnessRegistry::new());
        registry.register(provider);
        let profile = zuse_engine::EngineProfile::synced(path, "ws-queue", "user-bob");
        let profile = if durable {
            profile.with_durable_command_commits()
        } else {
            profile
        };
        let lock = zuse_engine::InstanceLock::acquire(path).unwrap();
        EngineCore::assemble_with_profile_locked(profile, registry, HarnessId::Mock, None, lock)
            .unwrap()
    }
    #[tokio::test]
    #[ignore = "requires cloud runtime acceptance feature"]
    async fn rust_cloud_queue_restores_selected_model_and_preserves_explicit_stop() {
        use zuse_rpc::{RpcService, methods};
        let temporary = tempfile::tempdir().unwrap();
        let data = temporary.path().join("runtime");
        let checkout = temporary.path().join("checkout");
        std::fs::create_dir_all(&data).unwrap();
        std::fs::create_dir(&checkout).unwrap();
        let provider = Arc::new(Provider {
            started: Arc::new(Notify::new()),
            release: Arc::new(Notify::new()),
            runs: Arc::new(AtomicUsize::new(0)),
            requests: Arc::new(std::sync::Mutex::new(Vec::new())),
            supports_steering: true,
        });
        let core = queued_cloud_engine(&data, provider.clone(), true);
        core.rpc_service().handle(methods::MUTATE,json!({"op":"createChat","chatId":"queue-chat","deviceId":core.device_id,"cwd":checkout,"config":{"harness":"mock","model":"initial-model","sandbox":"workspace-write"}})).await.unwrap();
        let command = |id: &str, model: &str| -> zuse_doc::SessionCommandEntry {
            serde_json::from_value(json!({"id":id,"issuedBy":format!("cloud-mailbox:hmac-sha256:{id}"),"issuedAt":chrono::Utc::now().timestamp_millis(),"status":"pending","payload":{"kind":"run","messageId":format!("message-{id}"),"request":{"prompt":format!("Prompt {id}"),"harness":"mock","model":model,"reasoning":"high","modelOptions":{"fast":"enabled"},"cwd":checkout,"sandbox":"workspace-write"}}})).unwrap()
        };
        core.doc_host
            .admit_mailbox_command("queue-chat", command("first", "queued-selected-model"))
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(5), provider.started.notified())
            .await
            .unwrap();
        core.doc_host
            .admit_mailbox_command("queue-chat", command("held", "queued-selected-model"))
            .await
            .unwrap();
        let handle = core.doc_host.open_local("queue-chat").unwrap();
        let mut queue = handle.watch_queue();
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if queue.borrow().iter().any(|row| row.id == "message-held") {
                    break;
                }
                queue.changed().await.unwrap();
            }
        })
        .await
        .unwrap();
        assert_eq!(
            handle
                .doc_arc()
                .queued_run_request("message-held")
                .unwrap()
                .unwrap()
                .model
                .as_deref(),
            Some("queued-selected-model")
        );
        assert_eq!(handle.doc_arc().queue_paused(), Some(false));
        let config: zuse_protocol::ChatConfig = serde_json::from_value(
            json!({"harness":"mock","model":"settings-changed-later","sandbox":"workspace-write"}),
        )
        .unwrap();
        core.workspace
            .set_chat_config("queue-chat", &config)
            .unwrap();
        drop(queue);
        drop(handle);
        core.shutdown().await;
        drop(core);
        let core = queued_cloud_engine(&data, provider.clone(), true);
        let handle = core.doc_host.open_local("queue-chat").unwrap();
        tokio::time::timeout(Duration::from_secs(5), provider.started.notified())
            .await
            .unwrap();
        assert_eq!(provider.runs.load(Ordering::SeqCst), 2);
        let request = provider.requests.lock().unwrap()[1].clone();
        assert_eq!(request.model.as_deref(), Some("queued-selected-model"));
        assert_eq!(request.reasoning, Some(ReasoningLevel::High));
        assert_eq!(request.model_options["fast"], "enabled");
        let paused_row = core
            .doc_host
            .queue_message("queue-chat", "Hold after Stop", vec![])
            .unwrap();
        let newer_config: zuse_protocol::ChatConfig = serde_json::from_value(
            json!({"harness":"mock","model":"settings-changed-again","sandbox":"workspace-write"}),
        )
        .unwrap();
        core.workspace
            .set_chat_config("queue-chat", &newer_config)
            .unwrap();
        assert!(
            !handle
                .doc_arc()
                .queued_request_is_held(&paused_row)
                .unwrap(),
            "An explicitly queued row is not a send held for the running turn"
        );
        core.rpc_service().handle(methods::RELAY_COMMAND,json!({"chatId":"queue-chat","entry":{"id":"explicit-stop","issuedBy":"cloud-ui","issuedAt":chrono::Utc::now().timestamp_millis(),"status":"pending","payload":{"kind":"interrupt"}}})).await.unwrap();
        assert_eq!(handle.doc_arc().queue_paused(), Some(true));
        assert!(
            handle
                .doc_arc()
                .read_queue()
                .unwrap()
                .iter()
                .any(|row| row.id == paused_row)
        );
        drop(handle);
        core.shutdown().await;
        drop(core);
        let core = queued_cloud_engine(&data, provider.clone(), true);
        let handle = core.doc_host.open_local("queue-chat").unwrap();
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(
            provider.runs.load(Ordering::SeqCst),
            2,
            "An explicit Stop must remain paused across restart"
        );
        assert!(
            handle
                .doc_arc()
                .read_queue()
                .unwrap()
                .iter()
                .any(|row| row.id == paused_row)
        );
        assert!(
            core.doc_host
                .send_queued_now("queue-chat", &paused_row)
                .await
                .unwrap()
        );
        tokio::time::timeout(Duration::from_secs(5), provider.started.notified())
            .await
            .unwrap();
        assert_eq!(provider.runs.load(Ordering::SeqCst), 3);
        assert_eq!(
            provider.requests.lock().unwrap()[2].model.as_deref(),
            Some("settings-changed-later")
        );
        provider.release.notify_one();
        tokio::time::timeout(Duration::from_secs(5), async {
            while core.sessions.turn_in_flight("queue-chat") {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(
            handle
                .doc_arc()
                .read_entries()
                .unwrap()
                .iter()
                .filter(|entry| entry.id == paused_row)
                .count(),
            1
        );
        drop(handle);
        core.shutdown().await;
        drop(core);
    }

    #[tokio::test]
    #[ignore = "requires engine runtime acceptance feature"]
    async fn rust_cloud_queue_changes_keep_local_reopen_conservative() {
        use zuse_rpc::{RpcService, methods};
        let temporary = tempfile::tempdir().unwrap();
        let data = temporary.path().join("runtime");
        let checkout = temporary.path().join("checkout");
        std::fs::create_dir_all(&data).unwrap();
        std::fs::create_dir(&checkout).unwrap();
        let provider = Arc::new(Provider {
            started: Arc::new(Notify::new()),
            release: Arc::new(Notify::new()),
            runs: Arc::new(AtomicUsize::new(0)),
            requests: Arc::new(std::sync::Mutex::new(Vec::new())),
            supports_steering: true,
        });
        let core = queued_cloud_engine(&data, provider.clone(), false);
        core.rpc_service().handle(methods::MUTATE,json!({"op":"createChat","chatId":"local-queue","deviceId":core.device_id,"cwd":checkout,"config":{"harness":"mock","model":"local-model","sandbox":"workspace-write"}})).await.unwrap();
        core.rpc_service().handle(methods::RELAY_COMMAND,json!({"chatId":"local-queue","entry":{"id":"local-run","issuedBy":"local-ui","issuedAt":chrono::Utc::now().timestamp_millis(),"status":"pending","payload":{"kind":"run","messageId":"local-message","request":{"prompt":"Start local work","harness":"mock","model":"local-model","cwd":checkout,"sandbox":"workspace-write"}}}})).await.unwrap();
        tokio::time::timeout(Duration::from_secs(5), provider.started.notified())
            .await
            .unwrap();
        let row = core
            .doc_host
            .queue_message("local-queue", "Keep queued after local restart", vec![])
            .unwrap();
        core.shutdown().await;
        drop(core);
        let core = queued_cloud_engine(&data, provider.clone(), false);
        let handle = core.doc_host.open_local("local-queue").unwrap();
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(
            provider.runs.load(Ordering::SeqCst),
            1,
            "Local restart still requires an explicit queue action"
        );
        assert!(
            handle
                .doc_arc()
                .read_queue()
                .unwrap()
                .iter()
                .any(|item| item.id == row)
        );
        assert!(
            core.doc_host
                .send_queued_now("local-queue", &row)
                .await
                .unwrap()
        );
        tokio::time::timeout(Duration::from_secs(5), provider.started.notified())
            .await
            .unwrap();
        assert_eq!(provider.runs.load(Ordering::SeqCst), 2);
        provider.release.notify_one();
        tokio::time::timeout(Duration::from_secs(5), async {
            while core.sessions.turn_in_flight("local-queue") {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
        drop(handle);
        core.shutdown().await;
        drop(core);
    }

    struct AllocatorFixture {
        machine: std::sync::Mutex<Option<zuse_cloud::providers::Machine>>,
        creates: AtomicUsize,
        launches: AtomicUsize,
        uncertain_create: std::sync::atomic::AtomicBool,
        pool: PgPool,
        fence_create: bool,
        directory: std::path::PathBuf,
        checkout: std::path::PathBuf,
        wrapper: std::path::PathBuf,
        child: tokio::sync::Mutex<Option<tokio::process::Child>>,
    }
    #[async_trait]
    impl zuse_cloud::runtime::allocation::AllocationProvider for AllocatorFixture {
        async fn find(
            &self,
            name: &str,
        ) -> Result<Option<zuse_cloud::providers::Machine>, zuse_cloud::providers::ProviderError>
        {
            Ok(self
                .machine
                .lock()
                .unwrap()
                .as_ref()
                .filter(|machine| machine.id == name || machine.name == name)
                .cloned())
        }
        async fn create(
            &self,
            _snapshot: &str,
            name: &str,
        ) -> Result<zuse_cloud::providers::Machine, zuse_cloud::providers::ProviderError> {
            self.creates.fetch_add(1, Ordering::SeqCst);
            let machine = zuse_cloud::providers::Machine {
                id: "allocated-guest".into(),
                name: name.into(),
                state: zuse_cloud::providers::MachineState::Running,
                vcpu: Some(2),
                memory_bytes: Some(8 << 30),
            };
            *self.machine.lock().unwrap() = Some(machine.clone());
            if self.fence_create {
                sqlx::query("UPDATE api_cloud_workspaces SET desired_state='deleted',state='deleting',revision=revision+1 WHERE workspace_id='ws-bob'").execute(&self.pool).await.unwrap();
            }
            if self.uncertain_create.swap(false, Ordering::SeqCst) {
                return Err(zuse_cloud::providers::ProviderError::OutcomeUnknown);
            }
            Ok(machine)
        }
        async fn launch(
            &self,
            _machine: &str,
            launch: &zuse_cloud::runtime::allocation::RuntimeLaunch,
        ) -> Result<(), zuse_cloud::providers::ProviderError> {
            self.launches.fetch_add(1, Ordering::SeqCst);
            let mut child = self.child.lock().await;
            if child.is_none() {
                let boot = self.directory.parent().unwrap().join("allocation-boot");
                std::fs::write(&boot, launch.boot_token.as_bytes()).unwrap();
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&boot, std::fs::Permissions::from_mode(0o600)).unwrap();
                *child = Some(
                    tokio::process::Command::new("bash")
                        .arg(concat!(
                            env!("CARGO_MANIFEST_DIR"),
                            "/../../infra/cloud/launch-runtime.sh"
                        ))
                        .arg("--foreground")
                        .env("ZUSE_CLOUD_WORKSPACE_ID", &launch.workspace_id)
                        .env("ZUSE_API_URL", &launch.api_url)
                        .env("ZUSE_RUNTIME_GENERATION", launch.generation.to_string())
                        .env("ZUSE_STORAGE_INCARNATION_ID", &launch.incarnation)
                        .env("ZUSE_RUNTIME_BOOT_TOKEN_FILE", boot)
                        .env("ZUSE_USER_DATA", &self.directory)
                        .env("ZUSE_CLOUD_WORKSPACE_ROOT", &self.checkout)
                        .env("ZUSE_RUNTIME_BINARY", &self.wrapper)
                        .env(
                            "ZUSE_WORKSPACE_RUNTIME_STATUS_DIR",
                            self.directory.parent().unwrap().join("status"),
                        )
                        .stdout(std::process::Stdio::null())
                        .stderr(
                            std::fs::File::create(
                                self.directory.parent().unwrap().join("allocated-guest.log"),
                            )
                            .unwrap(),
                        )
                        .kill_on_drop(true)
                        .spawn()
                        .unwrap(),
                );
            }
            // The guest starts, but the provider response is lost. Reconcile enrollment, not a new VM.
            Err(zuse_cloud::providers::ProviderError::OutcomeUnknown)
        }
    }
    #[async_trait]
    impl zuse_cloud::runtime::lifecycle::RetainedProvider for AllocatorFixture {
        async fn inspect_retained(
            &self,
            id: &str,
        ) -> Result<Option<zuse_cloud::providers::Machine>, zuse_cloud::providers::ProviderError>
        {
            Ok(self.machine.lock().unwrap().clone().filter(|m| m.id == id))
        }
        async fn pause_retained(
            &self,
            id: &str,
        ) -> Result<(), zuse_cloud::providers::ProviderError> {
            if let Some(mut child) = self.child.lock().await.take() {
                stop_guest(&mut child).await;
            }
            let mut machine = self.machine.lock().unwrap();
            let machine = machine
                .as_mut()
                .ok_or(zuse_cloud::providers::ProviderError::NotFound)?;
            assert_eq!(machine.id, id);
            machine.state = zuse_cloud::providers::MachineState::Paused;
            Ok(())
        }
        async fn wake_retained(
            &self,
            id: &str,
            _: &zuse_cloud::providers::MachineState,
        ) -> Result<(), zuse_cloud::providers::ProviderError> {
            let mut machine = self.machine.lock().unwrap();
            let machine = machine
                .as_mut()
                .ok_or(zuse_cloud::providers::ProviderError::NotFound)?;
            assert_eq!(machine.id, id);
            machine.state = zuse_cloud::providers::MachineState::Running;
            Ok(())
        }
        async fn launch_retained(
            &self,
            d: &zuse_protocol::cloud_lifecycle::Directive,
            workspace: &str,
            api: &str,
        ) -> Result<(), zuse_cloud::providers::ProviderError> {
            let mut child = self.child.lock().await;
            if child.is_none() {
                *child = Some(
                    tokio::process::Command::new("bash")
                        .arg(concat!(
                            env!("CARGO_MANIFEST_DIR"),
                            "/../../infra/cloud/launch-runtime.sh"
                        ))
                        .arg("--foreground")
                        .env("ZUSE_RUNTIME_RETAINED", "true")
                        .env("ZUSE_CLOUD_WORKSPACE_ID", workspace)
                        .env("ZUSE_API_URL", api)
                        .env("ZUSE_RUNTIME_GENERATION", d.generation.to_string())
                        .env("ZUSE_STORAGE_INCARNATION_ID", &d.storage_incarnation_id)
                        .env("ZUSE_USER_DATA", &self.directory)
                        .env("ZUSE_CLOUD_WORKSPACE_ROOT", &self.checkout)
                        .env("ZUSE_RUNTIME_BINARY", &self.wrapper)
                        .env(
                            "ZUSE_WORKSPACE_RUNTIME_STATUS_DIR",
                            self.directory.parent().unwrap().join("status"),
                        )
                        .stdout(std::process::Stdio::null())
                        .stderr(
                            std::fs::OpenOptions::new()
                                .create(true)
                                .append(true)
                                .open(self.directory.parent().unwrap().join("allocated-guest.log"))
                                .unwrap(),
                        )
                        .kill_on_drop(true)
                        .spawn()
                        .unwrap(),
                );
            }
            Ok(())
        }
    }
    async fn allocation_due(api: &GuestApi) {
        sqlx::query("UPDATE zuse_rust_jobs SET due_at=$1 WHERE kind='workspace-allocate' AND state='pending'").bind(chrono::Utc::now().timestamp_millis()).execute(&api.pool).await.unwrap();
    }
    async fn allocation_fixture(
        api: &GuestApi,
        root: &std::path::Path,
        fence_create: bool,
    ) -> AllocatorFixture {
        use std::os::unix::fs::PermissionsExt;
        let checkout = root.join("checkout");
        std::fs::create_dir(&checkout).unwrap();
        for args in [
            vec!["init", "--initial-branch=main"],
            vec!["config", "user.email", "fixture@example.invalid"],
            vec!["config", "user.name", "Fixture"],
        ] {
            assert!(
                std::process::Command::new("git")
                    .arg("-C")
                    .arg(&checkout)
                    .args(args)
                    .output()
                    .unwrap()
                    .status
                    .success()
            );
        }
        std::fs::write(checkout.join("tracked"), "prepared repository bytes").unwrap();
        for args in [
            vec!["add", "tracked"],
            vec!["commit", "-m", "Prepared fixture"],
            vec!["update-ref", "refs/remotes/origin/main", "HEAD"],
        ] {
            assert!(
                std::process::Command::new("git")
                    .arg("-C")
                    .arg(&checkout)
                    .args(args)
                    .output()
                    .unwrap()
                    .status
                    .success()
            );
        }
        let binary = std::env::var_os("ZUSE_CLOUD_TEST_GUEST_BIN").expect("Set guest binary");
        let wrapper = root.join("test-runtime");
        // Python exec passes the executable path as data, independent of shell quoting.
        let wrapper_text = format!(
            "#!/usr/bin/env python3\nimport os,sys\nos.environ.pop('ZUSE_API_URL',None)\nos.execv({}, [{}, '--test-harness', '--ipc-port', '0', *sys.argv[1:]])\n",
            serde_json::to_string(&binary.to_string_lossy()).unwrap(),
            serde_json::to_string(&binary.to_string_lossy()).unwrap()
        );
        std::fs::write(&wrapper, wrapper_text).unwrap();
        std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o700)).unwrap();
        sqlx::query("UPDATE api_cloud_workspaces SET state='queued',desired_state='ready',runtime_state='offline',provider_sandbox_id=NULL,runtime_boot_token_hash=NULL,runtime_boot_token_expires_at=NULL,runtime_credential_hash=NULL,request_config='{\"runtimeGeneration\":2,\"gatewayEpoch\":5,\"rustRepositoryCheckout\":{\"branch\":\"cloud/allocated\",\"baseRef\":\"main\"}}' WHERE workspace_id='ws-bob'").execute(&api.pool).await.unwrap();
        AllocatorFixture {
            machine: std::sync::Mutex::new(None),
            creates: AtomicUsize::new(0),
            launches: AtomicUsize::new(0),
            uncertain_create: std::sync::atomic::AtomicBool::new(!fence_create),
            pool: api.pool.clone(),
            fence_create,
            directory: root.join("data"),
            checkout,
            wrapper,
            child: tokio::sync::Mutex::new(None),
        }
    }
    #[tokio::test]
    #[ignore = "requires isolated PostgreSQL, guest binary and browser driver"]
    async fn rust_cloud_composer_launch_close_and_reopen() {
        let api = GuestApi::start_options_with_grants(true, false, false, true, true).await;
        sqlx::raw_sql(include_str!("fixtures/creation.sql"))
            .execute(&api.pool)
            .await
            .unwrap();
        sqlx::raw_sql("CREATE TABLE api_cloud_auth_authorities(account_id text PRIMARY KEY,storage_incarnation_id text NOT NULL,auth_epoch bigint NOT NULL,state text NOT NULL); INSERT INTO api_cloud_auth_authorities VALUES('user-bob','original-auth-storage',1,'ready'); UPDATE api_cloud_project_builds SET settings=settings||'{\"providerAuthDeliveryVersion\":1}'::jsonb WHERE build_id='rust-build';").execute(&api.pool).await.unwrap();
        sqlx::raw_sql(zuse_cloud::runtime::assets::MIGRATION)
            .execute(&api.pool)
            .await
            .unwrap();
        sqlx::query("UPDATE api_cloud_project_builds SET settings=settings||'{\"rustLaunchAssetsVersion\":1}'::jsonb WHERE build_id='rust-build'").execute(&api.pool).await.unwrap();
        if std::env::var_os("ZUSE_CLOUD_MODEL_ACCEPTANCE").is_some() {
            sqlx::query("UPDATE api_cloud_project_builds SET settings=settings||'{\"rustLaunchModelConfigVersion\":1}'::jsonb WHERE build_id='rust-build'").execute(&api.pool).await.unwrap();
        }
        if std::env::var_os("ZUSE_CLOUD_SOURCE_ACCEPTANCE").is_some() {
            sqlx::raw_sql("ALTER TABLE api_cloud_projects ADD COLUMN repository_identity text, ADD COLUMN repository_url text, ADD COLUMN display_name text, ADD COLUMN default_branch text; UPDATE api_cloud_projects SET repository_identity='github.com/example/project',repository_url='https://github.com/example/project.git',display_name='Source project',default_branch='main' WHERE project_id='create-project';").execute(&api.pool).await.unwrap();
        }
        let now = chrono::Utc::now().timestamp_millis();
        sqlx::query("INSERT INTO api_cloud_billing_periods(period_id,account_id,status,period_start,period_end,included_provider_cost_micros,markup_basis_points,overage_cap_micros) VALUES('launch-period','user-bob','active',$1,$2,100000000,0,0)").bind(now-1000).bind(now+3600000).execute(&api.pool).await.unwrap();
        let root = tempfile::Builder::new()
            .prefix("cloud-launch-")
            .tempdir_in(concat!(env!("CARGO_MANIFEST_DIR"), "/../../.context"))
            .unwrap()
            .keep();
        let provider = allocation_fixture(&api, root.as_path(), false).await;
        if std::env::var_os("ZUSE_CLOUD_SOURCE_ACCEPTANCE").is_some() {
            for args in [vec!["switch", "-c", "feature/from-pr"]] {
                assert!(
                    std::process::Command::new("git")
                        .arg("-C")
                        .arg(&provider.checkout)
                        .args(args)
                        .status()
                        .unwrap()
                        .success()
                );
            }
            std::fs::write(
                provider.checkout.join("tracked"),
                "selected Cloud source bytes",
            )
            .unwrap();
            for args in [
                vec!["commit", "-am", "Selected source"],
                vec!["update-ref", "refs/remotes/origin/feature/from-pr", "HEAD"],
                vec![
                    "update-ref",
                    "refs/remotes/origin/feature/available",
                    "HEAD",
                ],
                vec!["update-ref", "refs/pull/84/head", "HEAD"],
                vec!["switch", "main"],
            ] {
                assert!(
                    std::process::Command::new("git")
                        .arg("-C")
                        .arg(&provider.checkout)
                        .args(args)
                        .status()
                        .unwrap()
                        .success()
                );
            }
        }
        provider.uncertain_create.store(false, Ordering::SeqCst);
        let cli = root.as_path().join("claude-fixture");
        std::fs::write(&cli, include_str!("fixtures/claude-provider.py")).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&cli, std::fs::Permissions::from_mode(0o700)).unwrap();
        let binary = std::env::var("ZUSE_CLOUD_TEST_GUEST_BIN").unwrap();
        let wrapper = format!(
            "#!/usr/bin/env python3\nimport os,sys\nos.environ.pop('ZUSE_API_URL',None)\nos.environ['CLAUDE_CODE_EXECUTABLE']={}\nos.environ['ZUSE_TEST_PROVIDER_SEEN']={}\nos.execv({},[{},'--ipc-port','0',*sys.argv[1:]])\n",
            serde_json::to_string(&cli.to_string_lossy()).unwrap(),
            serde_json::to_string(&root.as_path().join("provider-observed").to_string_lossy())
                .unwrap(),
            serde_json::to_string(&binary).unwrap(),
            serde_json::to_string(&binary).unwrap()
        );
        std::fs::write(&provider.wrapper, wrapper).unwrap();
        let path = std::path::PathBuf::from(
            std::env::var("ZUSE_CLOUD_LAUNCH_UI_FIXTURE_FILE")
                .expect("Set browser fixture ready file"),
        );
        std::fs::write(
            path.with_extension("runtime"),
            root.to_string_lossy().as_bytes(),
        )
        .unwrap();
        std::fs::write(&path, &api.origin).unwrap();
        tokio::time::timeout(Duration::from_secs(if std::env::var_os("ZUSE_CLOUD_SOURCE_ACCEPTANCE").is_some() { 1800 } else { 900 }), async {
            while !path.with_extension("done").exists() {
                if path.with_extension("offline").exists() && !path.with_extension("offline-ready").exists() {
                    if let Some(mut child)=provider.child.lock().await.take() { stop_guest(&mut child).await; }
                    std::fs::write(path.with_extension("offline-ready"),"runtime stopped; account API remains online").unwrap();
                }
                if std::env::var_os("ZUSE_CLOUD_SYNC_ACCEPTANCE").is_some()
                    && path.with_extension("sync-repository").exists()
                    && !path.with_extension("sync-repository-ready").exists() {
                    // Import legacy catalog metadata after creation, without
                    // changing the enrolled runtime's original checkout config.
                    sqlx::raw_sql("ALTER TABLE api_cloud_projects ADD COLUMN IF NOT EXISTS repository_url text; UPDATE api_cloud_projects SET repository_url='https://github.com/fixture/sync-repository.git' WHERE project_id='create-project';")
                        .execute(&api.pool).await.unwrap();
                    std::fs::write(path.with_extension("sync-repository-ready"),"catalog metadata imported").unwrap();
                }
                allocation_due(&api).await;
                if let Err(error) = api
                    .control
                    .allocate_once(&provider, chrono::Utc::now().timestamp_millis())
                    .await
                {
                    std::fs::write(root.join("allocation-error"), error.code).unwrap();
                }
                if std::env::var_os("ZUSE_CLOUD_LIFECYCLE_ACCEPTANCE").is_some() {
                    if let Err(error)=api.control.lifecycle_once(&provider,chrono::Utc::now().timestamp_millis()).await {
                        std::fs::write(root.join("lifecycle-error"),error.code).unwrap();
                    }
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        })
        .await
        .expect("Cloud composer browser acceptance timed out");
        assert_eq!(provider.creates.load(Ordering::SeqCst), 1);
        assert_eq!(
            std::fs::read_to_string(root.as_path().join("provider-observed"))
                .unwrap()
                .lines()
                .count(),
            if std::env::var_os("ZUSE_CLOUD_UPLOAD_ACCEPTANCE").is_some() {
                2
            } else {
                1
            },
            "Only explicitly submitted turns may reach the provider"
        );
        if let Some(mut child) = provider.child.lock().await.take() {
            stop_guest(&mut child).await;
        }
        api.finish().await;
    }

    #[tokio::test]
    #[ignore = "requires isolated PostgreSQL and ZUSE_CLOUD_TEST_GUEST_BIN"]
    async fn rust_cloud_allocation_recovers_uncertain_create_and_launch_into_original_chat() {
        let api = GuestApi::start().await;
        let now = chrono::Utc::now().timestamp_millis();
        assert_eq!(
            api.control
                .queue_allocation("ws-bob", "prepared-fixture", now)
                .await
                .unwrap_err()
                .code,
            "cloud_allocation_retained_state"
        );
        let root = tempfile::tempdir().unwrap();
        let provider = allocation_fixture(&api, root.path(), false).await;
        let operation = api
            .control
            .queue_allocation("ws-bob", "prepared-fixture", now)
            .await
            .unwrap();
        assert_eq!(
            api.control
                .queue_allocation("ws-bob", "prepared-fixture", now)
                .await
                .unwrap(),
            operation
        );
        assert_eq!(
            api.control
                .queue_allocation("ws-bob", "different-image", now)
                .await
                .unwrap_err()
                .code,
            "cloud_allocation_identity_conflict"
        );
        let payload: String = sqlx::query_scalar(
            "SELECT payload::text FROM zuse_rust_jobs WHERE kind='workspace-allocate'",
        )
        .fetch_one(&api.pool)
        .await
        .unwrap();
        assert!(!payload.contains("rust_boot_"));
        assert_eq!(
            api.control
                .allocate_once(&provider, now)
                .await
                .unwrap_err()
                .code,
            "cloud_provider_outcome_unknown"
        );
        allocation_due(&api).await;
        // One claim adopts the uncertain machine and launches it; the fixture
        // loses that launch response too.
        assert_eq!(
            api.control
                .allocate_once(&provider, chrono::Utc::now().timestamp_millis())
                .await
                .unwrap_err()
                .code,
            "cloud_provider_outcome_unknown"
        );
        assert_eq!(provider.creates.load(Ordering::SeqCst), 1);
        assert_eq!(provider.launches.load(Ordering::SeqCst), 1);
        // An assigned source disappearing must not cause another create.
        let retained = provider.machine.lock().unwrap().take().unwrap();
        allocation_due(&api).await;
        assert_eq!(
            api.control
                .allocate_once(&provider, chrono::Utc::now().timestamp_millis())
                .await
                .unwrap_err()
                .code,
            "cloud_allocation_machine_missing"
        );
        assert_eq!(provider.creates.load(Ordering::SeqCst), 1);
        *provider.machine.lock().unwrap() = Some(retained);
        allocation_due(&api).await;
        assert_eq!(
            api.control
                .allocate_once(&provider, chrono::Utc::now().timestamp_millis())
                .await
                .unwrap_err()
                .code,
            "cloud_provider_outcome_unknown"
        );
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let online: bool = sqlx::query_scalar("SELECT runtime_state='online' FROM api_cloud_workspaces WHERE workspace_id='ws-bob'").fetch_one(&api.pool).await.unwrap();
                if online { break; }
                if let Some(status) = provider.child.lock().await.as_mut().unwrap().try_wait().unwrap() {
                    panic!("Allocated guest exited {status}: {}", std::fs::read_to_string(root.path().join("allocated-guest.log")).unwrap());
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        }).await.unwrap_or_else(|error|panic!("Allocated guest not ready: {error}: {}",std::fs::read_to_string(root.path().join("allocated-guest.log")).unwrap()));
        let client = api.client().await;
        let mut transcript = client
            .client
            .subscribe(
                zuse_rpc::methods::WATCH_DOC_MESSAGES,
                json!({"chatId":"session-bob"}),
            )
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let snapshot = transcript.recv().await.unwrap();
                if snapshot.to_string().contains("Cloud runtime fixture reply") {
                    break;
                }
            }
        })
        .await
        .unwrap();
        allocation_due(&api).await;
        assert!(
            api.control
                .allocate_once(&provider, chrono::Utc::now().timestamp_millis())
                .await
                .unwrap()
        );
        let record: Value = sqlx::query_scalar(
            "SELECT to_jsonb(a) FROM zuse_rust_allocations a WHERE operation_id=$1",
        )
        .bind(operation)
        .fetch_one(&api.pool)
        .await
        .unwrap();
        assert_eq!(record["phase"], "ready");
        let branch = std::process::Command::new("git")
            .arg("-C")
            .arg(&provider.checkout)
            .args(["branch", "--show-current"])
            .output()
            .unwrap();
        assert_eq!(
            String::from_utf8(branch.stdout).unwrap().trim(),
            "cloud/allocated"
        );
        assert!(record["sealed_boot_token"].is_null());
        assert_eq!(provider.creates.load(Ordering::SeqCst), 1);
        // The uncertain launch is redelivered unchanged to the same machine.
        assert_eq!(provider.launches.load(Ordering::SeqCst), 2);
        let receipt: Value = sqlx::query_scalar(
            "SELECT request_config FROM api_cloud_workspaces WHERE workspace_id='ws-bob'",
        )
        .fetch_one(&api.pool)
        .await
        .unwrap();
        assert_eq!(receipt["runtimeBootstrapReceipt"]["workspaceId"], "ws-bob");
        assert_eq!(
            receipt["rustStorageEnrollment"]["sourceSandboxId"],
            "allocated-guest"
        );
        assert_eq!(receipt["rustCompletedLaunchCommandId"], "original-launch");
        drop(transcript);
        client.closed.cancel();
        drop(client);
        stop_guest(provider.child.lock().await.as_mut().unwrap()).await;
        api.finish().await;
    }
    #[tokio::test]
    #[ignore = "requires isolated PostgreSQL and ZUSE_CLOUD_TEST_GUEST_BIN"]
    async fn rust_cloud_allocation_delete_race_preserves_provider_receipt_without_attaching_guest()
    {
        let api = GuestApi::start().await;
        let root = tempfile::tempdir().unwrap();
        let provider = allocation_fixture(&api, root.path(), true).await;
        let operation = api
            .control
            .queue_allocation(
                "ws-bob",
                "prepared-fixture",
                chrono::Utc::now().timestamp_millis(),
            )
            .await
            .unwrap();
        assert_eq!(
            api.control
                .allocate_once(&provider, chrono::Utc::now().timestamp_millis())
                .await
                .unwrap_err()
                .code,
            "cloud_allocation_fenced"
        );
        let row: Value = sqlx::query_scalar(
            "SELECT to_jsonb(w) FROM api_cloud_workspaces w WHERE workspace_id='ws-bob'",
        )
        .fetch_one(&api.pool)
        .await
        .unwrap();
        assert_eq!(row["desired_state"], "deleted");
        assert!(row["provider_sandbox_id"].is_null());
        let machine: String = sqlx::query_scalar(
            "SELECT machine_id FROM zuse_rust_allocations WHERE operation_id=$1",
        )
        .bind(operation)
        .fetch_one(&api.pool)
        .await
        .unwrap();
        assert_eq!(machine, "allocated-guest");
        assert_eq!(provider.launches.load(Ordering::SeqCst), 0);
        api.finish().await;
    }

    struct GrantFixtureIssuer {
        pool: PgPool,
        calls: AtomicUsize,
        revoke: std::sync::atomic::AtomicBool,
        blocked: std::sync::atomic::AtomicBool,
        entered: Notify,
        release: Notify,
    }
    #[async_trait]
    impl zuse_cloud::runtime::grants::GrantIssuer for GrantFixtureIssuer {
        async fn issue(
            &self,
            workspace: &str,
            provider: zuse_protocol::cloud_auth::CloudAuthProvider,
            _credential: &str,
            request: &zuse_protocol::cloud_auth::ProviderGrantRequest,
        ) -> Result<zuse_protocol::cloud_auth::SealedProviderGrant, ApiError> {
            use tokio::io::AsyncWriteExt;
            self.calls.fetch_add(1, Ordering::SeqCst);
            if self.blocked.load(Ordering::SeqCst) {
                self.entered.notify_one();
                self.release.notified().await;
            }
            let thumb =
                zuse_cloud::crypto::credential_thumbprint(&request.credential_public_jwk).unwrap();
            // Independent original Node crypto wire, rather than the Rust receiver's encrypt path.
            let script = r#"const c=require('node:crypto');let input='';process.stdin.on('data',b=>input+=b);process.stdin.on('end',()=>{const i=JSON.parse(input);const aad={protocolVersion:1,providerId:i.provider,requestId:i.request.requestId,keyThumbprint:i.thumb,authorityIncarnationId:'original-auth-storage',authorityEpoch:1};const payload={zuseAccountId:'user-bob',workspaceId:i.workspace,runtimeGeneration:i.request.runtimeGeneration,keyThumbprint:i.thumb,authorityIncarnationId:aad.authorityIncarnationId,authorityEpoch:aad.authorityEpoch,providerId:i.provider,method:'api-key',credentialKind:'api-key',secret:'fixture-provider-secret-never-persist',providerAccountId:null,issuer:null,issuedAt:Date.now(),expiresAt:Date.now()+1800000};const key=c.randomBytes(32),iv=c.randomBytes(12),cipher=c.createCipheriv('aes-256-gcm',key,iv);cipher.setAAD(Buffer.from(JSON.stringify(aad)));const ciphertext=Buffer.concat([cipher.update(JSON.stringify(payload)),cipher.final()]);const publicKey=c.createPublicKey({key:JSON.parse(i.request.credentialPublicJwk),format:'jwk'});process.stdout.write(JSON.stringify({...aad,wrappedKey:c.publicEncrypt({key:publicKey,oaepHash:'sha256',padding:c.constants.RSA_PKCS1_OAEP_PADDING},key).toString('base64url'),iv:iv.toString('base64url'),ciphertext:ciphertext.toString('base64url'),tag:cipher.getAuthTag().toString('base64url')}));});"#;
            let mut child = tokio::process::Command::new("node")
                .args(["-e", script])
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::null())
                .kill_on_drop(true)
                .spawn()
                .unwrap();
            child.stdin.take().unwrap().write_all(json!({"workspace":workspace,"provider":provider,"request":request,"thumb":thumb}).to_string().as_bytes()).await.unwrap();
            let output = child.wait_with_output().await.unwrap();
            assert!(output.status.success());
            if self.revoke.swap(false, Ordering::SeqCst) {
                sqlx::query("UPDATE api_cloud_workspaces SET runtime_credential_hash=NULL,desired_state='deleted',state='deleting' WHERE workspace_id=$1").bind(workspace).execute(&self.pool).await.unwrap();
            }
            Ok(serde_json::from_slice(&output.stdout).unwrap())
        }
    }
    fn no_secret_files(root: &std::path::Path) {
        for entry in std::fs::read_dir(root).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                no_secret_files(&path);
            } else {
                let bytes = std::fs::read(path).unwrap();
                assert!(
                    !bytes
                        .windows(b"fixture-provider-secret-never-persist".len())
                        .any(|bytes| bytes == b"fixture-provider-secret-never-persist")
                );
            }
        }
    }
    #[tokio::test]
    #[ignore = "requires isolated PostgreSQL, Node and ZUSE_CLOUD_TEST_GUEST_BIN"]
    async fn rust_cloud_provider_grant_reaches_claude_without_persisting_secret_and_fences_revocation()
     {
        use std::os::unix::fs::PermissionsExt;
        // The issuer needs the same isolated pool used by its API; it is installed below before serving.
        let api = GuestApi::start_with_grants().await;
        let issuer = api.grant_issuer.as_ref().unwrap();
        let now = chrono::Utc::now().timestamp_millis();
        sqlx::raw_sql("CREATE TABLE api_cloud_auth_authorities(account_id text PRIMARY KEY,storage_incarnation_id text NOT NULL,auth_epoch bigint NOT NULL,state text NOT NULL); INSERT INTO api_cloud_auth_authorities VALUES('user-bob','original-auth-storage',1,'ready');").execute(&api.pool).await.unwrap();
        sqlx::query("UPDATE api_cloud_workspaces SET request_config=jsonb_set(request_config,'{providerAuthMode}','\"broker-v1\"') WHERE workspace_id='ws-bob'").execute(&api.pool).await.unwrap();
        use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD as B64};
        let master = zuse_cloud::crypto::MasterKey::parse(&B64.encode([15; 32])).unwrap();
        let intent = json!({"commandId":"original-launch","turnId":"original-turn","title":"Brokered native turn","agent":"claude","model":"fixture","permissions":[],"runtimeMode":"approval-required","firstMessage":"Use account-owned provider credentials"});
        let cipher = master
            .seal(
                b"zuse-cloud-workspace-launch-intent-v1\nuser-bob\nws-bob",
                intent.to_string().as_bytes(),
            )
            .unwrap();
        sqlx::query("UPDATE api_cloud_workspace_launch_intents SET ciphertext=$1,expires_at=$2 WHERE workspace_id='ws-bob'").bind(cipher).bind(now+120000).execute(&api.pool).await.unwrap();
        let temporary = tempfile::tempdir().unwrap();
        let data = temporary.path().join("data");
        let checkout = temporary.path().join("checkout");
        std::fs::create_dir(&checkout).unwrap();
        let seen = temporary.path().join("provider-observed");
        let cli = temporary.path().join("claude-fixture");
        std::fs::write(&cli,r#"#!/usr/bin/env python3
import os,sys,json
if '--version' in sys.argv:
 print('2.1.228');sys.exit(0)
assert os.environ.get('ANTHROPIC_API_KEY')=='fixture-provider-secret-never-persist'
assert 'CLAUDE_CODE_OAUTH_TOKEN' not in os.environ
assert 'ANTHROPIC_AUTH_TOKEN' not in os.environ
for line in sys.stdin:
 m=json.loads(line)
 if m.get('type')=='control_request':
  print(json.dumps({'type':'control_response','response':{'subtype':'success','request_id':m['request_id'],'response':{'models':[{'value':'fixture','displayName':'Fixture'}],'commands':[]}}}),flush=True)
 elif m.get('type')=='user':
  if '--system-prompt' not in sys.argv:
   with open(os.environ['ZUSE_TEST_PROVIDER_SEEN'],'a') as f:f.write('credential-only-in-child\n')
  print(json.dumps({'type':'stream_event','event':{'type':'content_block_delta','delta':{'type':'text_delta','text':'Brokered native provider reply'}}}),flush=True)
  print(json.dumps({'type':'result','subtype':'success','result':'Brokered native provider reply','usage':{'input_tokens':1,'output_tokens':1},'session_id':'broker-native-session'}),flush=True)
  break
"#).unwrap();
        std::fs::set_permissions(&cli, std::fs::Permissions::from_mode(0o700)).unwrap();
        let boot = temporary.path().join("boot");
        std::fs::write(&boot, "packaged-guest-boot").unwrap();
        let log = temporary.path().join("guest.log");
        let binary = std::env::var_os("ZUSE_CLOUD_TEST_GUEST_BIN").unwrap();
        let spawn = |with_boot: bool| {
            let mut command = tokio::process::Command::new(&binary);
            command
                .args([
                    "--workspace-id",
                    "ws-bob",
                    "--api-url",
                    &api.origin,
                    "--generation",
                    "2",
                    "--storage-incarnation-id",
                    "packaged-storage",
                    "--ipc-port",
                    "0",
                ])
                .arg("--data-dir")
                .arg(&data)
                .arg("--workspace-root")
                .arg(&checkout)
                .env("CLAUDE_CODE_EXECUTABLE", &cli)
                .env("ZUSE_TEST_PROVIDER_SEEN", &seen)
                .env("ANTHROPIC_API_KEY", "wrong-inherited-account")
                .env("CLAUDE_CODE_OAUTH_TOKEN", "wrong-inherited-oauth")
                .env("ANTHROPIC_AUTH_TOKEN", "wrong-inherited-auth")
                .env_remove("ZUSE_RUNTIME_BOOT_TOKEN_FILE")
                .env_remove("ZUSE_WORKSPACE_RUNTIME_LOCK_FD")
                .env_remove("ZUSE_API_URL")
                .stdout(std::process::Stdio::null())
                .stderr(std::fs::File::create(&log).unwrap())
                .kill_on_drop(true);
            if with_boot {
                command.arg("--boot-token-file").arg(&boot);
            }
            command.spawn().unwrap()
        };
        issuer.blocked.store(true, Ordering::SeqCst);
        let mut child = spawn(true);
        tokio::time::timeout(Duration::from_secs(5), issuer.entered.notified())
            .await
            .unwrap();
        let client = api.client().await;
        let stop = tokio::time::Instant::now();
        client.client.call(zuse_rpc::methods::RELAY_COMMAND,json!({"chatId":"session-bob","entry":{"id":"stop-before-grant","issuedBy":"cloud-ui","issuedAt":chrono::Utc::now().timestamp_millis(),"status":"pending","payload":{"kind":"interrupt"}}})).await.unwrap();
        assert!(stop.elapsed() < Duration::from_secs(3));
        assert!(
            !seen.exists(),
            "A stopped startup must not reach the provider"
        );
        issuer.blocked.store(false, Ordering::SeqCst);
        issuer.release.notify_waiters();
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(
            !seen.exists(),
            "A late grant must not start the stopped turn"
        );
        // Preparation hands the gateway from its diagnostic connection to the
        // serving one; clients reconnect once, as the desktop does.
        client.closed.cancel();
        drop(client);
        let client = api.client().await;
        client.client.call(zuse_rpc::methods::RELAY_COMMAND,json!({"chatId":"session-bob","entry":{"id":"explicit-run-after-grant","issuedBy":"cloud-ui","issuedAt":chrono::Utc::now().timestamp_millis(),"status":"pending","payload":{"kind":"run","messageId":"explicit-provider-message","request":{"prompt":"Run with the account credential","harness":"claude-code","model":"fixture","cwd":checkout,"sandbox":"workspace-write","autoApprove":false}}}})).await.unwrap();
        client.closed.cancel();
        drop(client);
        let ran = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if seen.exists() {
                    break;
                }
                if let Some(status) = child.try_wait().unwrap() {
                    panic!(
                        "Grant guest failed {status}: {}",
                        std::fs::read_to_string(&log).unwrap()
                    );
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await;
        if let Err(error) = ran {
            let client = api.client().await;
            let mut transcript = client
                .client
                .subscribe(
                    zuse_rpc::methods::WATCH_DOC_MESSAGES,
                    json!({"chatId":"session-bob"}),
                )
                .await
                .unwrap();
            let snapshot = tokio::time::timeout(Duration::from_secs(3), transcript.recv()).await;
            panic!(
                "Provider did not run: {error}; issuer_calls={}; snapshot={snapshot:?}; log={}",
                issuer.calls.load(Ordering::SeqCst),
                std::fs::read_to_string(&log).unwrap()
            );
        }
        let client = api.client().await;
        let mut transcript = client
            .client
            .subscribe(
                zuse_rpc::methods::WATCH_DOC_MESSAGES,
                json!({"chatId":"session-bob"}),
            )
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if transcript
                    .recv()
                    .await
                    .unwrap()
                    .to_string()
                    .contains("Brokered native provider reply")
                {
                    break;
                }
            }
        })
        .await
        .unwrap();
        assert_eq!(std::fs::read_to_string(&seen).unwrap().lines().count(), 1);
        assert!(issuer.calls.load(Ordering::SeqCst) >= 1);
        no_secret_files(&data);
        no_secret_files(&checkout);
        drop(transcript);
        client.closed.cancel();
        drop(client);
        stop_guest(&mut child).await;
        let mut restarted = spawn(false);
        let client = api.client().await;
        let stored: Value = serde_json::from_slice(
            &std::fs::read(data.join("cloud-runtime-identity.json")).unwrap(),
        )
        .unwrap();
        let public = stored["credentialPublicJwk"].as_str().unwrap();
        let credential = stored["bootstrap"]["runtimeCredential"].as_str().unwrap();
        let endpoint = format!(
            "{}/v1/cloud/workspaces/ws-bob/runtime/providers/claude/grant",
            api.origin
        );
        let request = json!({"requestId":uuid::Uuid::new_v4().to_string(),"protocolVersion":1,"runtimeGeneration":2,"credentialPublicJwk":public,"reason":"initial"});
        // Runtime revocation after issuance invalidates the response before ciphertext leaves the API.
        issuer.revoke.store(true, Ordering::SeqCst);
        let (status, response) = runtime_post_for_url(&endpoint, credential, &request).await;
        assert_eq!(status, 401, "{response}");
        assert_eq!(response["error"], "workspace_runtime_fenced");
        assert!(response["ciphertext"].is_null());
        let status = tokio::time::timeout(Duration::from_secs(10), restarted.wait())
            .await
            .unwrap()
            .unwrap();
        assert!(!status.success());
        assert_eq!(
            std::fs::read_to_string(&seen).unwrap().lines().count(),
            1,
            "Reopen cannot replay the original turn"
        );
        no_secret_files(&data);
        client.closed.cancel();
        drop(client);
        api.finish().await;
    }

    struct GuestApi {
        pool: PgPool,
        admin: PgPool,
        schema: String,
        origin: String,
        control: Arc<zuse_cloud::runtime::RuntimeControl>,
        stop: tokio::sync::oneshot::Sender<()>,
        server: tokio::task::JoinHandle<()>,
        lost_ack_release: Arc<Notify>,
        grant_issuer: Option<Arc<GrantFixtureIssuer>>,
        projection_objects: Option<Arc<ProjectionObjects>>,
        quiesced_lost_ack: Arc<std::sync::atomic::AtomicBool>,
    }
    impl GuestApi {
        async fn start() -> Self {
            Self::start_with_mailbox(false).await
        }
        async fn start_with_mailbox(mailbox: bool) -> Self {
            Self::start_options(mailbox, true, mailbox).await
        }
        async fn start_options(mailbox: bool, launch_prompt: bool, withhold_ack: bool) -> Self {
            Self::start_options_with_grants(mailbox, launch_prompt, withhold_ack, false, false)
                .await
        }
        async fn start_with_grants() -> Self {
            Self::start_options_with_grants(false, true, false, true, false).await
        }
        async fn start_with_projections() -> Self {
            Self::start_options_with_grants(true, true, false, false, true).await
        }
        async fn start_options_with_grants(
            mailbox: bool,
            launch_prompt: bool,
            withhold_ack: bool,
            grants: bool,
            projections: bool,
        ) -> Self {
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
            sqlx::query("UPDATE api_cloud_workspaces SET runtime_boot_token_hash=$1,runtime_boot_token_expires_at=$2,request_config=$3 WHERE workspace_id='ws-bob'").bind(hash).bind(now+120000).bind(json!({"runtimeGeneration":2,"gatewayEpoch":5,"runtimeRpcProtocol":"zuse-rpc-v1","rustCommandMailboxEnabled":mailbox,"rustRuntimeStorage":{"format":"zuse-rust-engine-v1","storageIncarnationId":"packaged-storage"}})).execute(&pool).await.unwrap();
            let mut intent = json!({"commandId":"original-launch","turnId":"original-turn","title":"Guest launch","agent":"mock","model":"fixture","permissions":[],"runtimeMode":"approval-required"});
            if launch_prompt {
                intent["firstMessage"] = json!("Original guest prompt");
            }
            let ciphertext = master
                .seal(
                    b"zuse-cloud-workspace-launch-intent-v1\nuser-bob\nws-bob",
                    intent.to_string().as_bytes(),
                )
                .unwrap();
            sqlx::query("INSERT INTO api_cloud_workspace_launch_intents VALUES ('ws-bob','user-bob','chat-bob','session-bob','original-turn','original-launch',$1,$2,$3)").bind(ciphertext).bind(now+120000).bind(now).execute(&pool).await.unwrap();
            let grant_issuer = grants.then(|| {
                Arc::new(GrantFixtureIssuer {
                    pool: pool.clone(),
                    calls: AtomicUsize::new(0),
                    revoke: std::sync::atomic::AtomicBool::new(false),
                    blocked: std::sync::atomic::AtomicBool::new(false),
                    entered: Notify::new(),
                    release: Notify::new(),
                })
            });
            let projection_objects = projections.then(|| Arc::new(ProjectionObjects::default()));
            if projections {
                sqlx::raw_sql(include_str!("fixtures/transcript.sql"))
                    .execute(&pool)
                    .await
                    .unwrap();
                sqlx::query("UPDATE api_cloud_workspaces SET request_config=jsonb_set(request_config,'{rustNativeTranscriptEnabled}','true') WHERE workspace_id='ws-bob'").execute(&pool).await.unwrap();
            }
            let mut control = RuntimeControl::from_pool(pool.clone(), master, signer);
            if projections {
                control = control.with_creation_accounts(vec!["user-bob".into()]);
            }
            let mut store = Store::from_pool(pool.clone());
            if let Some(objects) = &projection_objects {
                control = control.with_transcript_objects(objects.clone());
                store = store.with_transcripts(
                    objects.clone(),
                    MasterKey::parse(&B64.encode([15; 32])).unwrap(),
                );
            }
            if let Some(issuer) = &grant_issuer {
                control = control.with_grant_issuer(issuer.clone());
            }
            let control = Arc::new(control);
            let app = router(
                AppState {
                    store,
                    identity: Arc::new(TestIdentity),
                    organizations_enabled: false,
                    organization_ids: Arc::new(vec![]),
                    runtime: Some(control.clone()),
                    gateway: Arc::new(zuse_cloud::gateway::Hub::default()),
                },
                vec![],
            );
            let lost_ack_release = Arc::new(Notify::new());
            let fail_first = Arc::new(std::sync::atomic::AtomicBool::new(withhold_ack));
            let projection_lost_ack = Arc::new(std::sync::atomic::AtomicBool::new(projections));
            let quiesced_lost_ack = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let app = app.layer(axum::middleware::from_fn({
                let release = lost_ack_release.clone();
                let quiesced_lost_ack = quiesced_lost_ack.clone();
                move |request: Request<Body>, next: axum::middleware::Next| {
                    let release = release.clone();
                    let fail = fail_first.clone();
                    let projection_lost_ack = projection_lost_ack.clone();
                    let quiesced_lost_ack = quiesced_lost_ack.clone();
                    async move {
                        if let Some(path) = std::env::var_os("ZUSE_CLOUD_TEST_HTTP_TRACE") {
                            use std::io::Write;
                            if request.uri().path().starts_with("/v1/cloud/workspaces/") {
                                let mut file = std::fs::OpenOptions::new()
                                    .create(true)
                                    .append(true)
                                    .open(path)
                                    .unwrap();
                                writeln!(file, "{} {}", request.method(), request.uri().path())
                                    .unwrap();
                            }
                        }
                        let (request, quiesced) = if quiesced_lost_ack.load(Ordering::SeqCst)
                            && request.uri().path().ends_with("/runtime/lifecycle")
                        {
                            let (parts, body) = request.into_parts();
                            let bytes = to_bytes(body, 1024 * 1024).await.unwrap();
                            let value: Value = serde_json::from_slice(&bytes).unwrap();
                            (
                                Request::from_parts(parts, Body::from(bytes)),
                                matches!(value["kind"].as_str(), Some("quiesced" | "parked")),
                            )
                        } else {
                            (request, false)
                        };
                        let projection = request
                            .uri()
                            .path()
                            .ends_with("/runtime/transcript-checkpoint");
                        let ack = request.uri().path().ends_with("/runtime/commands/ack");
                        let response = next.run(request).await;
                        if quiesced
                            && response.status().is_success()
                            && quiesced_lost_ack.swap(false, Ordering::SeqCst)
                        {
                            use axum::response::IntoResponse;
                            return (
                                StatusCode::SERVICE_UNAVAILABLE,
                                axum::Json(json!({"error":"simulated_lost_quiescence_ack"})),
                            )
                                .into_response();
                        }
                        if projection
                            && response.status().is_success()
                            && projection_lost_ack.swap(false, Ordering::SeqCst)
                        {
                            use axum::response::IntoResponse;
                            return (
                                StatusCode::SERVICE_UNAVAILABLE,
                                axum::Json(json!({"error":"simulated_lost_projection_ack"})),
                            )
                                .into_response();
                        }
                        if ack
                            && response.status().is_success()
                            && fail.swap(false, Ordering::SeqCst)
                        {
                            // Commit really happened. The guest is stopped before receiving this response.
                            release.notified().await;
                            use axum::response::IntoResponse;
                            (
                                StatusCode::SERVICE_UNAVAILABLE,
                                axum::Json(json!({"error":"simulated_lost_ack_response"})),
                            )
                                .into_response()
                        } else {
                            response
                        }
                    }
                }
            }));
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
                lost_ack_release,
                grant_issuer,
                projection_objects,
                quiesced_lost_ack,
            }
        }
        async fn finish(self) {
            self.lost_ack_release.notify_waiters();
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
    struct SignalOnDrop(Arc<Notify>);
    impl Drop for SignalOnDrop {
        fn drop(&mut self) {
            self.0.notify_one();
        }
    }
    struct ControlProvider {
        started: Arc<Notify>,
        stopped: Arc<Notify>,
        runs: Arc<AtomicUsize>,
        prompts: Arc<std::sync::Mutex<Vec<String>>>,
        steers: Arc<std::sync::Mutex<Vec<String>>>,
        answers: Arc<std::sync::Mutex<Vec<Vec<zuse_protocol::UserInputAnswer>>>>,
    }
    #[async_trait]
    impl Harness for ControlProvider {
        fn id(&self) -> HarnessId {
            HarnessId::Mock
        }
        fn display_name(&self) -> &str {
            "Native cloud control acceptance"
        }
        fn supports_steering(&self) -> bool {
            true
        }
        fn steering_mode(&self) -> zuse_protocol::SteeringMode {
            zuse_protocol::SteeringMode::StepBoundary
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
            mut controls: RunControls,
        ) -> Result<BoxStream<'static, Result<AgentEvent, HarnessError>>, HarnessError> {
            self.prompts.lock().unwrap().push(request.prompt.clone());
            self.runs.fetch_add(1, Ordering::SeqCst);
            self.started.notify_one();
            if request.prompt == "blocked-start" {
                let _stopped = SignalOnDrop(self.stopped.clone());
                controls.interrupt.cancelled().await;
                return Ok(futures::stream::iter([Ok(AgentEvent::Done {
                    status: DoneStatus::Interrupted,
                    result: None,
                    error: None,
                    session_id: None,
                })])
                .boxed());
            }
            let steers = self.steers.clone();
            let answers = self.answers.clone();
            let stream = futures::stream::iter([Ok(AgentEvent::TextDelta {
                text: "Awaiting native control".into(),
            })])
            .chain(
                futures::stream::once(async move {
                    let mut events = Vec::new();
                    if request.prompt == "question" || request.prompt.ends_with("\n\nquestion") {
                        let questions = vec![zuse_protocol::UserInputQuestion {
                            approval_prompt: None,
                            approval: None,
                            id: "choice".into(),
                            header: "Question".into(),
                            question: "Continue?".into(),
                            options: vec!["Continue".into()],
                            prefill: None,
                            multiline: false,
                            multi_select: false,
                        }];
                        let received = (controls.request_input)(questions)
                            .await
                            .unwrap_or_default();
                        answers.lock().unwrap().push(received);
                    } else {
                        let steer = controls
                            .steering
                            .recv()
                            .await
                            .expect("native steering delivery");
                        steers.lock().unwrap().push(steer.prompt);
                        if let Some(message_id) = steer.message_id {
                            events.push(AgentEvent::DeliveryAcknowledged { message_id });
                        }
                    }
                    events.push(AgentEvent::TextDelta {
                        text: "Native control completed".into(),
                    });
                    events
                        .into_iter()
                        .map(Ok::<_, HarnessError>)
                        .collect::<Vec<_>>()
                })
                .flat_map(futures::stream::iter),
            )
            .chain(futures::stream::iter([Ok(AgentEvent::Done {
                status: DoneStatus::Completed,
                result: None,
                error: None,
                session_id: None,
            })]))
            .boxed();
            Ok(stream)
        }
    }
    fn encrypted_native(
        key: &zuse_cloud_runtime::mailbox::CommandKey,
        id: &str,
        payload: Value,
        kind_override: Option<&str>,
    ) -> Value {
        let command: zuse_protocol::cloud_commands::CloudSessionCommand = serde_json::from_value(
            json!({"commandId":id,"sessionId":"session-bob","payload":payload}),
        )
        .unwrap();
        let plaintext = serde_json::to_vec(&command).unwrap();
        let kind = kind_override.unwrap_or(zuse_protocol::cloud_commands::session_command_kind(
            command.payload.kind(),
        ));
        let mut envelope:zuse_protocol::cloud_commands::CommandEnvelope=serde_json::from_value(json!({"protocolVersion":3,"workspaceId":"ws-bob","sessionId":"session-bob","commandId":id,"kind":kind,"fingerprint":key.fingerprint(&plaintext),"schemaVersion":1,"keyVersion":1,"destructionFence":0,"createdAt":chrono::Utc::now().timestamp_millis(),"iv":"pending","ciphertext":"pending","dependencies":[]})).unwrap();
        let (iv, ciphertext) = key.encrypt(&envelope, &plaintext).unwrap();
        envelope.iv = iv;
        envelope.ciphertext = ciphertext;
        serde_json::to_value(envelope).unwrap()
    }
    #[tokio::test]
    #[ignore = "requires isolated PostgreSQL and runtime acceptance feature"]
    async fn rust_cloud_native_mailbox_stops_stalled_start_and_delivers_steering_and_answers() {
        use zuse_cloud_runtime::{
            Authority, DataLock, Identity, IdentityBinding, Storage,
            mailbox::{CommandKey, Mailbox},
        };
        use zuse_rpc::{RpcService, methods};
        let api = GuestApi::start_options(true, false, false).await;
        let temporary = tempfile::tempdir().unwrap();
        let data = temporary.path().join("runtime");
        let checkout = temporary.path().join("checkout");
        std::fs::create_dir(&checkout).unwrap();
        let _lock = DataLock::acquire(&data, None).unwrap();
        let binding = IdentityBinding {
            workspace: "ws-bob".into(),
            api_url: api.origin.clone(),
            incarnation: "packaged-storage".into(),
            generation: 2,
        };
        let authority = Authority::new(&api.origin).unwrap();
        let mut identity = Identity::open(&data, &binding, Some("packaged-guest-boot")).unwrap();
        let boot = authority
            .enroll(&mut identity, &binding, Some("packaged-guest-boot"))
            .await
            .unwrap();
        let profile = zuse_engine::EngineProfile::synced(&data, "ws-bob", "user-bob")
            .with_durable_command_commits();
        let enrollment = authority.prepare_storage(&boot).await.unwrap();
        let mut storage = Storage::prepare(
            &data,
            &profile.store_root().join("docs.sqlite3"),
            &boot,
            "packaged-storage",
            &enrollment,
        )
        .unwrap();
        let provider = Arc::new(ControlProvider {
            started: Arc::new(Notify::new()),
            stopped: Arc::new(Notify::new()),
            runs: Arc::new(AtomicUsize::new(0)),
            prompts: Arc::new(std::sync::Mutex::new(Vec::new())),
            steers: Arc::new(std::sync::Mutex::new(Vec::new())),
            answers: Arc::new(std::sync::Mutex::new(Vec::new())),
        });
        let registry = Arc::new(HarnessRegistry::new());
        registry.register(provider.clone());
        let core = Arc::new(
            EngineCore::assemble_with_profile_locked(
                profile,
                registry,
                HarnessId::Mock,
                None,
                zuse_engine::InstanceLock::acquire(&data).unwrap(),
            )
            .unwrap(),
        );
        storage.commit().unwrap();
        // Simulate an existing cached bootstrap from a guest predating native commands.
        sqlx::query("UPDATE api_cloud_workspaces SET request_config=request_config #- '{runtimeBootstrapReceipt,capabilities}' WHERE workspace_id='ws-bob'")
            .execute(&api.pool).await.unwrap();
        let capability_url = format!("{}/v1/cloud/workspaces/ws-bob/data-key", api.origin);
        let before: Value = reqwest::Client::new()
            .get(&capability_url)
            .bearer_auth("bob")
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert!(before["sessionCommandVersion"].is_null());
        authority.commit_storage(&boot, &enrollment).await.unwrap();
        let after: Value = reqwest::Client::new()
            .get(&capability_url)
            .bearer_auth("bob")
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(after["sessionCommandVersion"], 1);
        core.rpc_service().handle(methods::MUTATE,json!({"op":"createChat","chatId":"chat-bob","deviceId":core.device_id,"cwd":checkout,"config":{"harness":"mock","model":"fixture","sandbox":"workspace-write"}})).await.unwrap();
        core.rpc_service()
            .handle(
                methods::CREATE_THREAD_SESSION,
                json!({"chatId":"session-bob","threadId":"chat-bob"}),
            )
            .await
            .unwrap();
        authority
            .launch_ready(&boot, "original-launch", 0)
            .await
            .unwrap();
        let encoded = identity
            .open_transcript_key(&boot.sealed_transcript_key)
            .unwrap();
        let mailbox = Arc::new(
            Mailbox::new(
                storage.database_path(),
                &boot,
                CommandKey::parse(&encoded).unwrap(),
                Arc::new(zuse_cloud_runtime::engine::EngineExecutor {
                    core: core.clone(),
                    thread: "chat-bob".into(),
                    checkout: checkout.clone(),
                    allow_test: true,
                    task_context: None,
                    progress: None,
                }),
            )
            .unwrap(),
        );
        let cancel = tokio_util::sync::CancellationToken::new();
        let (_send, receive) = tokio::sync::watch::channel(boot.clone());
        let worker = tokio::spawn({
            let mailbox = mailbox.clone();
            let cancel = cancel.clone();
            let authority = authority.clone();
            async move { mailbox.run(authority, receive, cancel).await }
        });
        let key = CommandKey::parse(&encoded).unwrap();
        let endpoint = format!("{}/v1/cloud/workspaces/ws-bob/commands", api.origin);
        let run = |prompt: &str, id: &str| json!({"kind":"run","messageId":format!("message-{id}"),"request":{"prompt":prompt,"harness":"mock","model":"fixture","cwd":checkout,"sandbox":"workspace-write"}});
        let first = encrypted_native(&key, "stalled", run("blocked-start", "stalled"), None);
        assert_eq!(runtime_post_for_url(&endpoint, "bob", &first).await.0, 202);
        tokio::time::timeout(Duration::from_secs(5), provider.started.notified())
            .await
            .unwrap();
        wait_command_state(&api, "stalled", "applied").await;
        let stop = encrypted_native(&key, "stop-stalled", json!({"kind":"interrupt"}), None);
        let start = tokio::time::Instant::now();
        assert_eq!(runtime_post_for_url(&endpoint, "bob", &stop).await.0, 202);
        tokio::time::timeout(Duration::from_secs(4), provider.stopped.notified())
            .await
            .unwrap();
        assert!(start.elapsed() < Duration::from_secs(4));
        wait_command_state(&api, "stop-stalled", "applied").await;
        tokio::time::timeout(Duration::from_secs(5), async {
            while core.sessions.turn_in_flight("session-bob") {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(
            runtime_post_for_url(
                &endpoint,
                "bob",
                &encrypted_native(&key, "steer-run", run("steering", "steer-run"), None)
            )
            .await
            .0,
            202
        );
        tokio::time::timeout(Duration::from_secs(5), provider.started.notified())
            .await
            .unwrap();
        let steer = encrypted_native(
            &key,
            "steer-command",
            json!({"kind":"steer","prompt":"Continue this way","messageId":"message-steer"}),
            None,
        );
        assert_eq!(runtime_post_for_url(&endpoint, "bob", &steer).await.0, 202);
        wait_command_state(&api, "steer-command", "applied").await;
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if !provider.steers.lock().unwrap().is_empty() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(runtime_post_for_url(&endpoint, "bob", &steer).await.0, 202);
        tokio::time::timeout(Duration::from_secs(5), async {
            while core.sessions.turn_in_flight("session-bob") {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(
            provider.steers.lock().unwrap().as_slice(),
            ["Continue this way"]
        );
        assert_eq!(
            runtime_post_for_url(
                &endpoint,
                "bob",
                &encrypted_native(&key, "question-run", run("question", "question-run"), None)
            )
            .await
            .0,
            202
        );
        let handle = core.doc_host.open_local("session-bob").unwrap();
        let request_id = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                for entry in handle.doc_arc().read_entries().unwrap() {
                    for part in entry.parts {
                        if let zuse_doc::MessagePart::Input {
                            id,
                            resolved: false,
                            ..
                        } = part
                        {
                            return id;
                        }
                    }
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap_or_else(|error|panic!("Question did not reach transcript: {error}; runs={}; worker_finished={}; prompts={:?}; session={:?}; entries={:?}",provider.runs.load(Ordering::SeqCst),worker.is_finished(),provider.prompts.lock().unwrap(),core.sessions.session_status("session-bob"),handle.doc_arc().read_entries().unwrap()));
        let answer = encrypted_native(
            &key,
            "answer-command",
            json!({"kind":"respondInput","requestId":request_id,"answers":[{"questionId":"choice","labels":["Continue"],"text":null}]}),
            None,
        );
        assert_eq!(runtime_post_for_url(&endpoint, "bob", &answer).await.0, 202);
        wait_command_state(&api, "answer-command", "applied").await;
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if !provider.answers.lock().unwrap().is_empty() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(runtime_post_for_url(&endpoint, "bob", &answer).await.0, 202);
        assert_eq!(provider.answers.lock().unwrap().len(), 1);
        assert_eq!(provider.answers.lock().unwrap()[0][0].labels, ["Continue"]);
        // An authenticated envelope cannot label a chargeable Run as a free Stop.
        let wrong = encrypted_native(
            &key,
            "wrong-kind",
            run("blocked-start", "wrong-kind"),
            Some("zuse.session.interrupt"),
        );
        assert_eq!(runtime_post_for_url(&endpoint, "bob", &wrong).await.0, 202);
        wait_command_state(&api, "wrong-kind", "rejected").await;
        assert_eq!(provider.runs.load(Ordering::SeqCst), 3);
        let category: Option<String> = sqlx::query_scalar(
            "SELECT category FROM zuse_rust_mailbox_commands WHERE command_id='wrong-kind'",
        )
        .fetch_one(&api.pool)
        .await
        .unwrap();
        assert_eq!(category.as_deref(), Some("command-kind-mismatch"));
        cancel.cancel();
        worker.await.unwrap().unwrap();
        drop(mailbox);
        drop(handle);
        core.shutdown().await;
        drop(core);
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
    fn encrypted_mailbox_message(
        key: &zuse_cloud_runtime::mailbox::CommandKey,
        id: &str,
        prompt: &str,
    ) -> Value {
        use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD as B64};
        let payload = json!({"commandId":id,"sessionId":"session-bob","clientMessageId":format!("message-{id}"),"text":prompt});
        let plaintext = payload.to_string();
        let mut envelope:zuse_protocol::cloud_commands::CommandEnvelope=serde_json::from_value(json!({"protocolVersion":3,"workspaceId":"ws-bob","sessionId":"session-bob","commandId":id,"kind":"messages.send","fingerprint":key.fingerprint(plaintext.as_bytes()),"schemaVersion":1,"keyVersion":1,"destructionFence":0,"createdAt":chrono::Utc::now().timestamp_millis(),"iv":B64.encode([0;12]),"ciphertext":"pending","dependencies":[]})).unwrap();
        let (iv, ciphertext) = key.encrypt(&envelope, plaintext.as_bytes()).unwrap();
        envelope.iv = iv;
        envelope.ciphertext = ciphertext;
        serde_json::to_value(envelope).unwrap()
    }
    async fn wait_command_state(api: &GuestApi, id: &str, state: &str) {
        tokio::time::timeout(Duration::from_secs(20),async {
            loop {
                let current:Option<String>=sqlx::query_scalar("SELECT state FROM zuse_rust_mailbox_commands WHERE workspace_id='ws-bob' AND command_id=$1").bind(id).fetch_optional(&api.pool).await.unwrap();
                if current.as_deref()==Some(state){break;}
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        }).await.unwrap();
    }
    #[tokio::test]
    #[ignore = "requires isolated PostgreSQL and ZUSE_CLOUD_TEST_GUEST_BIN"]
    async fn rust_cloud_guest_mailbox_accepts_before_start_and_recovers_lost_ack_and_receipt() {
        use zuse_cloud_runtime::mailbox::CommandKey;
        let api = GuestApi::start_with_mailbox(true).await;
        let client = reqwest::Client::new();
        let key_response = client
            .get(format!(
                "{}/v1/cloud/workspaces/ws-bob/data-key",
                api.origin
            ))
            .bearer_auth("bob")
            .send()
            .await
            .unwrap();
        assert_eq!(key_response.status(), reqwest::StatusCode::OK);
        let key_response: Value = key_response.json().await.unwrap();
        let key = CommandKey::parse(key_response["encodedKey"].as_str().unwrap()).unwrap();
        let endpoint = format!("{}/v1/cloud/workspaces/ws-bob/commands", api.origin);
        for (id, prompt) in [
            ("mailbox-first", "First durable message"),
            ("mailbox-second", "Second durable message"),
        ] {
            assert_eq!(
                runtime_post_for_url(
                    &endpoint,
                    "bob",
                    &encrypted_mailbox_message(&key, id, prompt)
                )
                .await
                .0,
                202
            );
        }
        let temporary = tempfile::tempdir().unwrap();
        let directory = temporary.path().join("runtime");
        let checkout = temporary.path().join("checkout");
        std::fs::create_dir(&checkout).unwrap();
        let boot = temporary.path().join("boot");
        std::fs::write(&boot, "packaged-guest-boot").unwrap();
        let log = temporary.path().join("guest.log");
        let mut child = guest_command(&api, &directory, &checkout, Some(&boot), &log)
            .spawn()
            .unwrap();
        wait_command_state(&api, "mailbox-first", "applied").await;
        // The first ACK response is still withheld. Stop with its durable receipt unacknowledged locally.
        stop_guest(&mut child).await;
        api.lost_ack_release.notify_waiters();
        let db_path = directory.join("orgs/ws-bob/user-bob/docs.sqlite3");
        let db = rusqlite_for_acceptance(&db_path);
        let pending:i64=db.query_row("SELECT COUNT(*) FROM zuse_cloud_command_receipts WHERE delivered=0 AND acknowledgment IS NOT NULL",[],|row|row.get(0)).unwrap();
        assert_eq!(pending, 1);
        drop(db);
        let mut child = guest_command(&api, &directory, &checkout, None, &log)
            .spawn()
            .unwrap();
        wait_command_state(&api, "mailbox-second", "applied").await;
        let connection = api.client().await;
        let mut messages = connection
            .client
            .subscribe(
                zuse_rpc::methods::WATCH_DOC_MESSAGES,
                json!({"chatId":"session-bob"}),
            )
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(10), async {
            let mut entries = Vec::new();
            loop {
                let snapshot: zuse_doc::TranscriptUpdate =
                    serde_json::from_value(messages.recv().await.unwrap()).unwrap();
                zuse_doc::transcript_delta::apply_transcript_frame(&mut entries, snapshot.frame)
                    .unwrap();
                if entries
                    .iter()
                    .any(|entry| entry.id == "message-mailbox-first")
                    && entries
                        .iter()
                        .any(|entry| entry.id == "message-mailbox-second")
                {
                    assert_eq!(
                        entries
                            .iter()
                            .filter(|entry| entry.id == "message-mailbox-first")
                            .count(),
                        1
                    );
                    assert_eq!(
                        entries
                            .iter()
                            .filter(|entry| entry.id == "message-mailbox-second")
                            .count(),
                        1
                    );
                    break;
                }
            }
        })
        .await
        .unwrap();
        drop(messages);
        connection.closed.cancel();
        drop(connection);
        // Fail exactly the receipt commit after the canonical engine queue admission.
        let db = rusqlite_for_acceptance(&db_path);
        db.execute_batch("CREATE TRIGGER fail_receipt BEFORE UPDATE OF acknowledgment ON zuse_cloud_command_receipts WHEN NEW.acknowledgment IS NOT NULL BEGIN SELECT RAISE(FAIL,'receipt failure fixture'); END;").unwrap();
        drop(db);
        assert_eq!(
            runtime_post_for_url(
                &endpoint,
                "bob",
                &encrypted_mailbox_message(&key, "mailbox-receipt-gap", "Recover admission proof")
            )
            .await
            .0,
            202
        );
        let status = tokio::time::timeout(Duration::from_secs(15), child.wait())
            .await
            .unwrap()
            .unwrap();
        assert!(
            !status.success(),
            "A failed durable receipt must stop its consumer"
        );
        let db = rusqlite_for_acceptance(&db_path);
        let unfinished:i64=db.query_row("SELECT COUNT(*) FROM zuse_cloud_command_receipts WHERE command_id='mailbox-receipt-gap' AND acknowledgment IS NULL",[],|row|row.get(0)).unwrap();
        assert_eq!(unfinished, 1);
        db.execute_batch("DROP TRIGGER fail_receipt;").unwrap();
        drop(db);
        let mut recovered = guest_command(&api, &directory, &checkout, None, &log)
            .spawn()
            .unwrap();
        // Queue admission is not proof of a terminal outcome. Cold recovery
        // quarantines the lost receipt instead of claiming it applied.
        wait_command_state(&api, "mailbox-receipt-gap", "outcome-unknown").await;
        let connection = api.client().await;
        let mut messages = connection
            .client
            .subscribe(
                zuse_rpc::methods::WATCH_DOC_MESSAGES,
                json!({"chatId":"session-bob"}),
            )
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(10), async {
            let mut entries = Vec::new();
            loop {
                let snapshot: zuse_doc::TranscriptUpdate =
                    serde_json::from_value(messages.recv().await.unwrap()).unwrap();
                zuse_doc::transcript_delta::apply_transcript_frame(&mut entries, snapshot.frame)
                    .unwrap();
                if entries
                    .iter()
                    .any(|entry| entry.id == "message-mailbox-receipt-gap")
                {
                    assert_eq!(
                        entries
                            .iter()
                            .filter(|entry| entry.id == "message-mailbox-receipt-gap")
                            .count(),
                        1
                    );
                    break;
                }
            }
        })
        .await
        .unwrap();
        drop(messages);
        connection.closed.cancel();
        drop(connection);
        stop_guest(&mut recovered).await;
        let db = rusqlite_for_acceptance(&db_path);
        for id in ["mailbox-first", "mailbox-second", "mailbox-receipt-gap"] {
            let processed: i64 = db
                .query_row(
                    "SELECT COUNT(*) FROM processed_commands WHERE command_id=?1",
                    [id],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(
                processed, 1,
                "Canonical engine claim must not be duplicated"
            );
        }
        drop(db);
        api.finish().await;
    }
    fn rusqlite_for_acceptance(path: &std::path::Path) -> rusqlite::Connection {
        let db = rusqlite::Connection::open_with_flags(
            path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE,
        )
        .unwrap();
        db.busy_timeout(Duration::from_secs(5)).unwrap();
        db
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
        assert!(stored["bootstrap"]["launchIntent"].is_null());
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
    struct RetainedFixture {
        state: std::sync::Mutex<zuse_cloud::providers::MachineState>,
        pauses: AtomicUsize,
        wakes: AtomicUsize,
        launches: AtomicUsize,
        fail_pause: std::sync::atomic::AtomicBool,
    }
    #[async_trait]
    impl zuse_cloud::runtime::lifecycle::RetainedProvider for RetainedFixture {
        async fn inspect_retained(
            &self,
            id: &str,
        ) -> Result<Option<zuse_cloud::providers::Machine>, zuse_cloud::providers::ProviderError>
        {
            assert_eq!(id, "machine-bob");
            Ok(Some(zuse_cloud::providers::Machine {
                id: id.into(),
                name: "original-machine".into(),
                state: self.state.lock().unwrap().clone(),
                vcpu: None,
                memory_bytes: None,
            }))
        }
        async fn pause_retained(
            &self,
            id: &str,
        ) -> Result<(), zuse_cloud::providers::ProviderError> {
            assert_eq!(id, "machine-bob");
            self.pauses.fetch_add(1, Ordering::SeqCst);
            // The provider accepted the mutation but its response can be lost.
            // Reconciliation must observe this state without issuing another pause.
            *self.state.lock().unwrap() = zuse_cloud::providers::MachineState::Paused;
            if self.fail_pause.load(Ordering::SeqCst) {
                return Err(zuse_cloud::providers::ProviderError::Unavailable);
            }
            Ok(())
        }
        async fn wake_retained(
            &self,
            id: &str,
            _: &zuse_cloud::providers::MachineState,
        ) -> Result<(), zuse_cloud::providers::ProviderError> {
            assert_eq!(id, "machine-bob");
            self.wakes.fetch_add(1, Ordering::SeqCst);
            *self.state.lock().unwrap() = zuse_cloud::providers::MachineState::Running;
            Ok(())
        }
        async fn launch_retained(
            &self,
            d: &zuse_protocol::cloud_lifecycle::Directive,
            workspace: &str,
            _: &str,
        ) -> Result<(), zuse_cloud::providers::ProviderError> {
            assert_eq!(d.source_machine_id, "machine-bob");
            assert_eq!(d.storage_incarnation_id, "packaged-storage");
            assert_eq!(workspace, "ws-bob");
            assert_eq!(d.generation, 2);
            self.launches.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }
    #[tokio::test]
    #[ignore = "requires isolated localhost PostgreSQL and built guest"]
    async fn rust_cloud_archive_quiesces_retains_and_lazily_resumes_original_machine() {
        use zuse_cloud::providers::MachineState;
        let api = GuestApi::start_with_projections().await;
        api.projection_objects
            .as_ref()
            .unwrap()
            .fail
            .store(true, Ordering::SeqCst);
        sqlx::raw_sql(include_str!("fixtures/creation.sql"))
            .execute(&api.pool)
            .await
            .unwrap();
        let now = chrono::Utc::now().timestamp_millis();
        sqlx::query("INSERT INTO api_cloud_billing_periods(period_id,account_id,status,period_start,period_end,included_provider_cost_micros,markup_basis_points,overage_cap_micros) VALUES('retained-period','user-bob','active',$1,$2,1000,500,500)").bind(now-1000).bind(now+86_400_000).execute(&api.pool).await.unwrap();
        let temp = tempfile::tempdir().unwrap();
        let data = temp.path().join("runtime");
        let checkout = temp.path().join("checkout");
        std::fs::create_dir(&checkout).unwrap();
        std::fs::write(checkout.join(".env"), "RETAINED_USER_FILE=yes").unwrap();
        std::fs::write(checkout.join("dirty.txt"), "original changes").unwrap();
        let boot = temp.path().join("boot");
        std::fs::write(&boot, "packaged-guest-boot").unwrap();
        let log = temp.path().join("guest.log");
        let mut child = guest_command(&api, &data, &checkout, Some(&boot), &log)
            .spawn()
            .unwrap();
        tokio::time::timeout(Duration::from_secs(20),async {
            loop {
                if let Some(status)=child.try_wait().unwrap(){panic!("Guest failed {status}: {}",std::fs::read_to_string(&log).unwrap());}
                let complete:bool=sqlx::query_scalar("SELECT NOT EXISTS(SELECT 1 FROM api_cloud_workspace_launch_intents WHERE workspace_id='ws-bob')").fetch_one(&api.pool).await.unwrap();
                if complete {break;}tokio::time::sleep(Duration::from_millis(20)).await;
            }
        }).await.unwrap();
        let already_ready = runtime_post_for_url(
            &format!("{}/v1/cloud/workspaces/ws-bob/resume", api.origin),
            "bob",
            &json!({"workspaceId":"ws-bob","commandId":"already-online","recoverRuntime":false}),
        )
        .await;
        assert_eq!(already_ready.0, 200, "{}", already_ready.1);
        let client = api.client().await;
        let mut transcript = client
            .client
            .subscribe(
                zuse_rpc::methods::WATCH_DOC_MESSAGES,
                json!({"chatId":"session-bob"}),
            )
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if transcript
                    .recv()
                    .await
                    .unwrap()
                    .to_string()
                    .contains("Cloud runtime fixture reply")
                {
                    break;
                }
            }
        })
        .await
        .unwrap();
        drop(transcript);
        client.closed.cancel();
        drop(client);
        tokio::time::timeout(Duration::from_secs(10), async {
            while api
                .projection_objects
                .as_ref()
                .unwrap()
                .attempts
                .lock()
                .unwrap()
                .is_empty()
            {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
        let pending = api
            .projection_objects
            .as_ref()
            .unwrap()
            .attempts
            .lock()
            .unwrap()[0]
            .clone();
        stop_guest(&mut child).await;
        api.projection_objects
            .as_ref()
            .unwrap()
            .fail
            .store(false, Ordering::SeqCst);
        child = guest_command(&api, &data, &checkout, None, &log)
            .spawn()
            .unwrap();
        let recovering = zuse_cloud_projection::reader::Reader::new(&api.origin).unwrap();
        tokio::time::timeout(Duration::from_secs(20), async {
            loop {
                if let Some(status) = child.try_wait().unwrap() {
                    panic!(
                        "Projection recovery failed {status}: {}",
                        std::fs::read_to_string(&log).unwrap()
                    );
                }
                if let Ok(Some(head)) = recovering.head("bob", "ws-bob", "session-bob").await {
                    if serde_json::to_string(&head.transcript)
                        .unwrap()
                        .contains("Cloud runtime fixture reply")
                    {
                        break;
                    }
                }
                tokio::time::sleep(Duration::from_millis(30)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(
            api.projection_objects
                .as_ref()
                .unwrap()
                .bytes
                .lock()
                .unwrap()
                .get(&pending.0)
                .unwrap(),
            &pending.1,
            "Restart must replay the original encrypted outbox bytes"
        );
        let client = api.client().await;
        client
            .client
            .call(
                zuse_rpc::methods::CREATE_THREAD_SESSION,
                json!({"chatId":"cached-independent-tab","threadId":"chat-bob"}),
            )
            .await
            .unwrap();
        client.client.call(zuse_rpc::methods::RELAY_COMMAND,json!({"chatId":"cached-independent-tab","entry":{"id":"independent-archive-run","issuedBy":"cloud-ui","issuedAt":chrono::Utc::now().timestamp_millis(),"status":"pending","payload":{"kind":"run","messageId":"independent-archive-message","request":{"prompt":"Independent cached conversation","harness":"mock","model":"fixture","cwd":checkout,"sandbox":"workspace-write","autoApprove":false}}}})).await.unwrap();
        let terminal = client
            .client
            .call(
                zuse_rpc::methods::OPEN_TERMINAL,
                json!({"chatId":"session-bob","cols":80,"rows":24}),
            )
            .await
            .unwrap();
        let pid_file = checkout.join("terminal.pid");
        client.client.call(zuse_rpc::methods::WRITE_TERMINAL,json!({"terminalId":terminal["id"],"data":format!("echo $$ > '{}'; exec sleep 300\n",pid_file.display())})).await.unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            while !pid_file.exists() {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
        let terminal_pid: i32 = std::fs::read_to_string(&pid_file)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        let endpoint = format!("{}/v1/cloud/workspaces/ws-bob/archive", api.origin);
        let body = json!({"workspaceId":"ws-bob","commandId":"archive-once"});
        assert_eq!(runtime_post_for_url(&endpoint, "alice", &body).await.0, 404);
        sqlx::query("UPDATE api_cloud_workspaces SET request_config=jsonb_set(request_config,'{rustStorageEnrollment,capabilities}',$1) WHERE workspace_id='ws-bob'").bind(json!(["rust-rpc-v1","rust-session-commands-v1"])).execute(&api.pool).await.unwrap();
        assert_eq!(runtime_post_for_url(&endpoint, "bob", &body).await.0, 409);
        sqlx::query("UPDATE api_cloud_workspaces SET request_config=jsonb_set(request_config,'{rustStorageEnrollment,capabilities}',$1) WHERE workspace_id='ws-bob'").bind(json!(["rust-rpc-v1","rust-session-commands-v1","rust-workspace-lifecycle-v1","rust-transcript-projection-v1"])).execute(&api.pool).await.unwrap();
        let (status, archiving) = runtime_post_for_url(&endpoint, "bob", &body).await;
        assert_eq!(status, 200, "{archiving}");
        assert_eq!(archiving["state"], "archiving");
        let archive_at = chrono::Utc::now().timestamp_millis();
        let provider = RetainedFixture {
            state: std::sync::Mutex::new(MachineState::Running),
            pauses: AtomicUsize::new(0),
            wakes: AtomicUsize::new(0),
            launches: AtomicUsize::new(0),
            fail_pause: std::sync::atomic::AtomicBool::new(false),
        };
        // No guest receipt yet: an early worker must leave the provider alone.
        api.control
            .lifecycle_once(&provider, archive_at)
            .await
            .unwrap();
        assert_eq!(provider.pauses.load(Ordering::SeqCst), 0);
        let status = tokio::time::timeout(Duration::from_secs(20), child.wait())
            .await
            .unwrap()
            .unwrap();
        assert!(
            status.success(),
            "{}",
            std::fs::read_to_string(&log).unwrap()
        );
        client.closed.cancel();
        drop(client);
        assert_eq!(
            unsafe { libc::kill(terminal_pid, 0) },
            -1,
            "The checkout terminal must stop before quiescence is acknowledged"
        );
        let original: Value = sqlx::query_scalar(
            "SELECT to_jsonb(w) FROM api_cloud_workspaces w WHERE workspace_id='ws-bob'",
        )
        .fetch_one(&api.pool)
        .await
        .unwrap();
        assert_eq!(
            original["request_config"]["rustLifecycleOperation"]["phase"],
            "quiesced"
        );
        assert_eq!(
            original["archive_delete_at"].as_i64().unwrap()
                - original["archive_requested_at"].as_i64().unwrap(),
            zuse_protocol::cloud::ARCHIVE_RETENTION_MS
        );
        let retry = runtime_post_for_url(&endpoint, "bob", &body).await;
        assert_eq!(retry.0, 200);
        assert_eq!(
            runtime_post_for_url(
                &format!("{}/v1/cloud/workspaces/ws-bob/pause", api.origin),
                "bob",
                &body
            )
            .await
            .0,
            409
        );
        // Provider failure is visible and leaves the original machine recoverable.
        provider.fail_pause.store(true, Ordering::SeqCst);
        sqlx::query("UPDATE zuse_rust_jobs SET due_at=0 WHERE kind='workspace-lifecycle'")
            .execute(&api.pool)
            .await
            .unwrap();
        assert!(
            api.control
                .lifecycle_once(&provider, chrono::Utc::now().timestamp_millis())
                .await
                .is_err()
        );
        let state: String = sqlx::query_scalar(
            "SELECT state FROM api_cloud_workspaces WHERE workspace_id='ws-bob'",
        )
        .fetch_one(&api.pool)
        .await
        .unwrap();
        assert_eq!(state, "archiving");
        provider.fail_pause.store(false, Ordering::SeqCst);
        sqlx::query("UPDATE zuse_rust_jobs SET state='failed',attempts=max_attempts WHERE kind='workspace-lifecycle'").execute(&api.pool).await.unwrap();
        assert_eq!(runtime_post_for_url(&endpoint, "bob", &body).await.0, 200);
        let attempts: i32 = sqlx::query_scalar(
            "SELECT attempts FROM zuse_rust_jobs WHERE kind='workspace-lifecycle'",
        )
        .fetch_one(&api.pool)
        .await
        .unwrap();
        assert_eq!(attempts, 0);
        for _ in 0..2 {
            sqlx::query("UPDATE zuse_rust_jobs SET due_at=0 WHERE kind='workspace-lifecycle'")
                .execute(&api.pool)
                .await
                .unwrap();
            api.control
                .lifecycle_once(&provider, chrono::Utc::now().timestamp_millis())
                .await
                .unwrap();
        }
        let archived: Value = reqwest::Client::new()
            .get(format!(
                "{}/v1/cloud/workspaces/ws-bob/archive-status",
                api.origin
            ))
            .bearer_auth("bob")
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(archived["archived"], true);
        assert_eq!(archived["machineRetained"], true);
        assert_eq!(archived["backupVerified"], false);
        assert_eq!(archived["lifecyclePhase"], "paused");
        assert!(archived["operationId"].is_string());
        let cache = zuse_cloud_projection::reader::Reader::new(&api.origin).unwrap();
        let cached = cache
            .head("bob", "ws-bob", "session-bob")
            .await
            .unwrap()
            .unwrap();
        assert!(cached.chat.archived);
        assert!(cached.queue_paused);
        let independent = cache
            .head("bob", "ws-bob", "cached-independent-tab")
            .await
            .unwrap()
            .unwrap();
        assert!(independent.chat.archived);
        assert!(
            serde_json::to_string(&independent.transcript)
                .unwrap()
                .contains("Independent cached conversation")
        );
        let wire:Value=reqwest::Client::new().get(format!("{}/v1/cloud/workspaces/ws-bob/sessions/session-bob/transcript-checkpoint?format=zuse-rust-transcript-v1",api.origin)).bearer_auth("bob").send().await.unwrap().json().await.unwrap();
        let independent_decoded = decrypt_projection(&wire["checkpoint"]);
        assert_eq!(
            independent_decoded["payloadFormat"],
            zuse_cloud_projection::FORMAT
        );
        assert!(
            independent_decoded["transcript"]
                .to_string()
                .contains("Original guest prompt")
        );
        let cached_json = serde_json::to_string(&cached.transcript).unwrap();
        assert!(
            cached_json.contains("Original guest prompt")
                && cached_json.contains("Cloud runtime fixture reply")
        );
        let root = cache
            .head("bob", "ws-bob", "chat-bob")
            .await
            .unwrap()
            .unwrap();
        assert!(root.chat.archived);
        assert!(root.conversations.iter().any(|c| c.id == "session-bob"));
        let legacy = reqwest::Client::new()
            .get(format!(
                "{}/v1/cloud/workspaces/ws-bob/sessions/session-bob/transcript-checkpoint",
                api.origin
            ))
            .bearer_auth("bob")
            .send()
            .await
            .unwrap();
        assert_eq!(legacy.status(), 409);
        let raw = api
            .projection_objects
            .as_ref()
            .unwrap()
            .bytes
            .lock()
            .unwrap()
            .values()
            .find(|bytes| bytes.len() > 10)
            .cloned()
            .unwrap();
        assert!(
            !String::from_utf8(raw)
                .unwrap()
                .contains("Original guest prompt")
        );
        // Expiry is not a retirement authorization, and reading does not schedule a wake.
        assert!(
            !api.control
                .lifecycle_once(
                    &provider,
                    archive_at + zuse_protocol::cloud::ARCHIVE_RETENTION_MS + 1
                )
                .await
                .unwrap()
        );
        let counts = (
            provider.wakes.load(Ordering::SeqCst),
            provider.launches.load(Ordering::SeqCst),
        );
        assert_eq!(counts, (0, 0));
        // Optional local UI acceptance harness: the actual API, encrypted objects,
        // PostgreSQL and archived guest fixture stay alive until the browser finishes.
        // This is test-only and uses the existing synthetic `bob` identity.
        if let Ok(path) = std::env::var("ZUSE_CLOUD_UI_FIXTURE_FILE") {
            let path = std::path::PathBuf::from(path);
            std::fs::write(&path, &api.origin).unwrap();
            tokio::time::timeout(Duration::from_secs(1800), async {
                while !path.with_extension("done").exists() {
                    tokio::time::sleep(Duration::from_millis(200)).await;
                }
            })
            .await
            .expect("browser archive acceptance timed out");
            let state: String = sqlx::query_scalar(
                "SELECT state FROM api_cloud_workspaces WHERE workspace_id='ws-bob'",
            )
            .fetch_one(&api.pool)
            .await
            .unwrap();
            assert_eq!(
                state, "paused",
                "browser Unarchive must leave compute paused"
            );
            assert_eq!(provider.wakes.load(Ordering::SeqCst), 0);
            assert_eq!(provider.launches.load(Ordering::SeqCst), 0);
            api.finish().await;
            return;
        }
        let (status, paused) = runtime_post_for_url(
            &format!("{}/v1/cloud/workspaces/ws-bob/unarchive", api.origin),
            "bob",
            &json!({"workspaceId":"ws-bob","commandId":"unarchive-once"}),
        )
        .await;
        assert_eq!(status, 200, "{paused}");
        assert_eq!(paused["state"], "paused");
        assert_eq!(paused["desiredState"], "paused");
        assert_eq!(provider.wakes.load(Ordering::SeqCst), 0);
        // Re-archive asleep starts a new ten-day window without launching anything.
        let (status, _) = runtime_post_for_url(
            &endpoint,
            "bob",
            &json!({"workspaceId":"ws-bob","commandId":"archive-again"}),
        )
        .await;
        assert_eq!(status, 200);
        let deadline: i64 = sqlx::query_scalar(
            "SELECT archive_delete_at FROM api_cloud_workspaces WHERE workspace_id='ws-bob'",
        )
        .fetch_one(&api.pool)
        .await
        .unwrap();
        assert!(deadline > original["archive_delete_at"].as_i64().unwrap());
        runtime_post_for_url(
            &format!("{}/v1/cloud/workspaces/ws-bob/unarchive", api.origin),
            "bob",
            &json!({"workspaceId":"ws-bob","commandId":"unarchive-again"}),
        )
        .await;
        sqlx::query("UPDATE api_cloud_billing_periods SET status='billing-hold'")
            .execute(&api.pool)
            .await
            .unwrap();
        let resume = format!("{}/v1/cloud/workspaces/ws-bob/resume", api.origin);
        let resume_body = json!({"workspaceId":"ws-bob","commandId":"resume-once"});
        assert_eq!(
            runtime_post_for_url(&resume, "bob", &resume_body).await.0,
            403
        );
        assert_eq!(provider.wakes.load(Ordering::SeqCst), 0);
        sqlx::query("UPDATE api_cloud_billing_periods SET status='active'")
            .execute(&api.pool)
            .await
            .unwrap();
        let resumed = runtime_post_for_url(&resume, "bob", &resume_body).await;
        assert_eq!(resumed.0, 200, "{}", resumed.1);
        for _ in 0..2 {
            sqlx::query("UPDATE zuse_rust_jobs SET due_at=0 WHERE kind='workspace-lifecycle'")
                .execute(&api.pool)
                .await
                .unwrap();
            api.control
                .lifecycle_once(&provider, chrono::Utc::now().timestamp_millis())
                .await
                .unwrap();
        }
        assert_eq!(provider.wakes.load(Ordering::SeqCst), 1);
        assert_eq!(provider.launches.load(Ordering::SeqCst), 1);
        let mut resumed = guest_command(&api, &data, &checkout, None, &log)
            .spawn()
            .unwrap();
        tokio::time::timeout(Duration::from_secs(20),async {loop {
            if let Some(status)=resumed.try_wait().unwrap(){panic!("Resume failed {status}: {}",std::fs::read_to_string(&log).unwrap());}
            let ready:bool=sqlx::query_scalar("SELECT state='ready' AND request_config->'rustLifecycleOperation'->>'phase'='ready' FROM api_cloud_workspaces WHERE workspace_id='ws-bob'").fetch_one(&api.pool).await.unwrap();if ready {break;}tokio::time::sleep(Duration::from_millis(25)).await;
        }}).await.unwrap();
        let client = api.client().await;
        let mut messages = client
            .client
            .subscribe(
                zuse_rpc::methods::WATCH_DOC_MESSAGES,
                json!({"chatId":"session-bob"}),
            )
            .await
            .unwrap();
        let history = tokio::time::timeout(Duration::from_secs(5), messages.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(history.to_string().contains("Original guest prompt"));
        assert!(history.to_string().contains("Cloud runtime fixture reply"));
        assert_eq!(
            std::fs::read_to_string(checkout.join("dirty.txt")).unwrap(),
            "original changes"
        );
        assert_eq!(
            std::fs::read_to_string(checkout.join(".env")).unwrap(),
            "RETAINED_USER_FILE=yes"
        );
        let final_row: Value = sqlx::query_scalar(
            "SELECT to_jsonb(w) FROM api_cloud_workspaces w WHERE workspace_id='ws-bob'",
        )
        .fetch_one(&api.pool)
        .await
        .unwrap();
        assert_eq!(
            final_row["provider_sandbox_id"],
            original["provider_sandbox_id"]
        );
        assert_eq!(
            final_row["wrapped_transcript_key"],
            original["wrapped_transcript_key"]
        );
        assert_eq!(final_row["chat_id"], original["chat_id"]);
        assert_eq!(
            final_row["request_config"]["runtimeCredentialKeyThumbprint"],
            original["request_config"]["runtimeCredentialKeyThumbprint"]
        );
        client.client.call(zuse_rpc::methods::RELAY_COMMAND,json!({"chatId":"session-bob","entry":{"id":"continue-after-archive","issuedBy":"cloud-ui","issuedAt":chrono::Utc::now().timestamp_millis(),"status":"pending","payload":{"kind":"run","messageId":"archive-continued-message","request":{"prompt":"Continue the original conversation after archive","harness":"mock","model":"fixture","cwd":checkout,"sandbox":"workspace-write","autoApprove":false}}}})).await.unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if messages
                    .recv()
                    .await
                    .unwrap()
                    .to_string()
                    .contains("Continue the original conversation after archive")
                {
                    break;
                }
            }
        })
        .await
        .unwrap();
        drop(messages);
        client.closed.cancel();
        drop(client);
        stop_guest(&mut resumed).await;
        api.finish().await;
    }
}

#[tokio::test]
#[ignore = "requires explicit isolated localhost PostgreSQL"]
async fn mailbox_durable_ordering_receipts_and_uncertain_lease_fences() {
    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD as B64};
    use ed25519_dalek::SigningKey;
    use sha2::{Digest, Sha256};
    use zuse_cloud::{crypto::MasterKey, runtime::RuntimeControl, tickets::TicketSigner};
    use zuse_protocol::cloud_commands::{AcknowledgmentState, LeaseRequest, RuntimeAcknowledgment};
    let (admin, pool, schema) = fixture().await;
    let now = chrono::Utc::now().timestamp_millis();
    let key = SigningKey::from_bytes(&[17; 32]);
    let jwk = json!({"kty":"OKP","crv":"Ed25519","d":B64.encode([17;32]),"x":B64.encode(key.verifying_key().as_bytes())});
    let signer = TicketSigner::from_jwk(&jwk.to_string(), "https://api-rust.zuse.sh").unwrap();
    let master = MasterKey::parse(&B64.encode([18; 32])).unwrap();
    let (transcript, wrapped) = master.create_transcript_key("user-bob", "ws-bob").unwrap();
    let credential = "mailbox-runtime";
    let hash: String = Sha256::digest(credential.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    sqlx::query("INSERT INTO zuse_rust_control_owners VALUES ('workspace','ws-bob','rust',2,$1)")
        .bind(now)
        .execute(&pool)
        .await
        .unwrap();
    let config = json!({"runtimeGeneration":2,"runtimeCredentialExpiresAtMs":now+300000,"runtimeRpcProtocol":"zuse-rpc-v1","rustCommandMailboxEnabled":true,"rustRuntimeStorage":{"format":"zuse-rust-engine-v1","storageIncarnationId":"original-storage"},"runtimeBootstrapReceipt":{"workspaceId":"ws-bob","generation":2,"gatewayEpoch":2},"runtimeCredentialKeyThumbprint":"fixture-rsa","runtimeSigningKeyThumbprint":"fixture-ed","rustStorageEnrollment":{"phase":"committed","generation":2,"sourceSandboxId":"machine-bob","storageIncarnationId":"original-storage","credentialKeyThumbprint":"fixture-rsa","signingKeyThumbprint":"fixture-ed"}});
    sqlx::query("UPDATE api_cloud_workspaces SET runtime_credential_hash=$1,wrapped_transcript_key=$2,request_config=$3 WHERE workspace_id='ws-bob'").bind(hash).bind(wrapped).bind(config).execute(&pool).await.unwrap();
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
    let path = "/v1/cloud/workspaces/ws-bob/commands";
    let envelope = |id: &str, session: &str| json!({"protocolVersion":3,"workspaceId":"ws-bob","sessionId":session,"commandId":id,"kind":"messages.send","fingerprint":format!("hmac-sha256:{}",B64.encode([19;32])),"schemaVersion":1,"keyVersion":1,"destructionFence":0,"createdAt":now,"iv":B64.encode([20;12]),"ciphertext":B64.encode([21;64]),"dependencies":[],"actor":{"subject":"forged-admin"}});
    let a = envelope("command-a", "session-a");
    assert_eq!(
        runtime_post(&app, path, "alice", &a).await.0,
        StatusCode::NOT_FOUND
    );
    let (status, first) = runtime_post(&app, path, "bob", &a).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(first["commandId"], "command-a");
    assert_eq!(runtime_post(&app, path, "bob", &a).await.1, first);
    let mut conflict = a.clone();
    conflict["fingerprint"] = json!(format!("hmac-sha256:{}", B64.encode([22; 32])));
    assert_eq!(
        runtime_post(&app, path, "bob", &conflict).await.0,
        StatusCode::CONFLICT
    );
    for (id, session) in [
        ("command-b", "session-a"),
        ("command-c", "session-c"),
        ("command-cancel", "session-cancel"),
    ] {
        assert_eq!(
            runtime_post(&app, path, "bob", &envelope(id, session))
                .await
                .0,
            StatusCode::ACCEPTED
        );
    }
    let wake_jobs: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM zuse_rust_jobs WHERE kind='mailbox-wake'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        wake_jobs, 4,
        "Retries must not duplicate durable wake responsibility"
    );
    // A scheduling failure must roll back acceptance, not strand an accepted command.
    sqlx::query("ALTER TABLE zuse_rust_jobs ADD CONSTRAINT reject_test_wakes CHECK (kind<>'mailbox-wake') NOT VALID").execute(&pool).await.unwrap();
    assert_eq!(
        runtime_post(
            &app,
            path,
            "bob",
            &envelope("not-accepted", "session-rollback")
        )
        .await
        .0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    let stranded: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM zuse_rust_mailbox_commands WHERE command_id='not-accepted'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(stranded, 0);
    sqlx::query("ALTER TABLE zuse_rust_jobs DROP CONSTRAINT reject_test_wakes")
        .execute(&pool)
        .await
        .unwrap();
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("{path}/command-cancel"))
                .header("authorization", "Bearer bob")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let cancelled: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 100000).await.unwrap()).unwrap();
    assert_eq!(cancelled["state"], "cancelled");
    let (_, key) = request(&app, "/v1/cloud/workspaces/ws-bob/data-key", "bob", None).await;
    assert_eq!(key["encodedKey"], transcript.as_str());
    assert_eq!(key["mailboxEnabled"], true);
    // A workspace without a guest can acquire its first encryption key; retries preserve it.
    sqlx::query("INSERT INTO api_cloud_workspaces SELECT (jsonb_populate_record(NULL::api_cloud_workspaces,to_jsonb(w)||'{\"workspace_id\":\"ws-preboot\",\"wrapped_transcript_key\":null}')).* FROM api_cloud_workspaces w WHERE workspace_id='ws-bob'").execute(&pool).await.unwrap();
    sqlx::query(
        "INSERT INTO zuse_rust_control_owners VALUES ('workspace','ws-preboot','rust',2,$1)",
    )
    .bind(now)
    .execute(&pool)
    .await
    .unwrap();
    let preboot = request(
        &app,
        "/v1/cloud/workspaces/ws-preboot/data-key",
        "bob",
        None,
    )
    .await;
    assert_eq!(preboot.0, StatusCode::OK);
    assert_eq!(
        B64.decode(preboot.1["encodedKey"].as_str().unwrap())
            .unwrap()
            .len(),
        32
    );
    assert_eq!(
        request(
            &app,
            "/v1/cloud/workspaces/ws-preboot/data-key",
            "bob",
            None
        )
        .await
        .1,
        preboot.1
    );

    let stored: Value = sqlx::query_scalar(
        "SELECT envelope FROM zuse_rust_mailbox_commands WHERE command_id='command-a'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(stored.get("actor").is_none());
    assert!(stored.get("plaintext").is_none());
    let lease_request = LeaseRequest {
        control_only: false,
        max_items: None,
        storage_incarnation_id: "original-storage".into(),
        wait_ms: None,
        after_revision: None,
    };
    let lease = control
        .command_lease("ws-bob", credential, &lease_request, now)
        .await
        .unwrap();
    assert_eq!(lease.leases.len(), 2);
    assert_eq!(lease.leases[0].command.command_id, "command-a");
    assert_eq!(lease.leases[1].command.command_id, "command-c");
    assert!(
        control
            .command_lease("ws-bob", credential, &lease_request, now + 1)
            .await
            .unwrap()
            .leases
            .is_empty()
    );
    let ack = RuntimeAcknowledgment {
        command_id: "command-a".into(),
        lease_token: lease.leases[0].lease_token.clone(),
        fingerprint: a["fingerprint"].as_str().unwrap().into(),
        state: AcknowledgmentState::Applied,
        category: None,
        result_iv: Some(B64.encode([0; 12])),
        result_ciphertext: Some(B64.encode([1; 16])),
    };
    let accepted = control
        .command_ack("ws-bob", credential, &ack, now + 2)
        .await
        .unwrap();
    assert_eq!(accepted["state"], "applied");
    assert_eq!(
        control
            .command_ack("ws-bob", credential, &ack, now + 3)
            .await
            .unwrap(),
        accepted
    );
    let second = control
        .command_lease("ws-bob", credential, &lease_request, now + 3)
        .await
        .unwrap();
    assert_eq!(second.leases.len(), 1);
    assert_eq!(second.leases[0].command.command_id, "command-b");
    let expired = control
        .command_lease("ws-bob", credential, &lease_request, now + 31000)
        .await
        .unwrap();
    assert!(expired.fence_required);
    assert!(expired.leases.is_empty());
    // Expired lease with a definitive receipt may finish; it must not be reissued by a live generation.
    let late = RuntimeAcknowledgment {
        command_id: "command-b".into(),
        lease_token: second.leases[0].lease_token.clone(),
        ..ack.clone()
    };
    assert_eq!(
        control
            .command_ack("ws-bob", credential, &late, now + 32000)
            .await
            .unwrap()["state"],
        "applied"
    );
    // Advancing the owned generation authorizes retry only against the same durable storage.
    sqlx::query("UPDATE zuse_rust_control_owners SET generation=3 WHERE resource_id='ws-bob'")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE api_cloud_workspaces SET request_config=jsonb_set(request_config,'{runtimeGeneration}','3') WHERE workspace_id='ws-bob'").execute(&pool).await.unwrap();
    assert_eq!(
        control
            .command_lease("ws-bob", credential, &lease_request, now + 33000)
            .await
            .err()
            .unwrap()
            .code,
        "workspace_runtime_fenced"
    );
    let credential = "mailbox-runtime-generation-3";
    let hash: String = Sha256::digest(credential.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    sqlx::query("UPDATE api_cloud_workspaces SET runtime_credential_hash=$1,request_config=request_config||'{\"runtimeBootstrapReceipt\":{\"workspaceId\":\"ws-bob\",\"generation\":3,\"gatewayEpoch\":3}}' WHERE workspace_id='ws-bob'").bind(hash).execute(&pool).await.unwrap();
    sqlx::query("UPDATE api_cloud_workspaces SET request_config=jsonb_set(request_config,'{rustStorageEnrollment,generation}','3') WHERE workspace_id='ws-bob'").execute(&pool).await.unwrap();
    let retry = control
        .command_lease("ws-bob", credential, &lease_request, now + 33000)
        .await
        .unwrap();
    assert_eq!(retry.leases.len(), 1);
    assert_eq!(retry.leases[0].command.command_id, "command-c");
    let stale = RuntimeAcknowledgment {
        command_id: "command-c".into(),
        lease_token: lease.leases[1].lease_token.clone(),
        ..ack.clone()
    };
    assert_eq!(
        control
            .command_ack("ws-bob", credential, &stale, now + 34000)
            .await
            .unwrap_err()
            .code,
        "stale-runtime-lease"
    );
    sqlx::query("UPDATE api_cloud_workspaces SET request_config=jsonb_set(request_config,'{rustRuntimeStorage,storageIncarnationId}','\"replacement-storage\"') WHERE workspace_id='ws-bob'").execute(&pool).await.unwrap();
    sqlx::query("UPDATE api_cloud_workspaces SET request_config=jsonb_set(request_config,'{rustStorageEnrollment,storageIncarnationId}','\"replacement-storage\"') WHERE workspace_id='ws-bob'").execute(&pool).await.unwrap();
    let changed = LeaseRequest {
        storage_incarnation_id: "replacement-storage".into(),
        ..lease_request
    };
    let replaced = control
        .command_lease("ws-bob", credential, &changed, now + 34000)
        .await
        .unwrap();
    assert!(replaced.leases.is_empty());
    assert!(!replaced.fence_required);
    assert_eq!(
        request(&app, &format!("{path}/command-c"), "bob", None)
            .await
            .1["state"],
        "outcome-unknown"
    );
    let watch = request(&app, &format!("{path}/watch?afterRevision=0"), "bob", None)
        .await
        .1;
    assert_eq!(watch["changes"].as_array().unwrap().len(), 4);
    // Long polling waits outside the transaction and wakes on a committed enqueue.
    let after = watch["nextRevision"].as_i64().unwrap();
    let waiting_app = app.clone();
    let watch_task = tokio::spawn(async move {
        request(
            &waiting_app,
            &format!(
                "/v1/cloud/workspaces/ws-bob/commands/watch?afterRevision={after}&waitMs=2000"
            ),
            "bob",
            None,
        )
        .await
    });
    tokio::time::sleep(std::time::Duration::from_millis(30)).await;
    assert!(!watch_task.is_finished());
    assert_eq!(
        runtime_post(&app, path, "bob", &envelope("command-wake", "session-wake"))
            .await
            .0,
        StatusCode::ACCEPTED
    );
    let woke = tokio::time::timeout(std::time::Duration::from_millis(500), watch_task)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(woke.0, StatusCode::OK);
    assert_eq!(woke.1["changes"][0]["commandId"], "command-wake");
    // A pending destructive fence cancels never-leased work rather than executing stale bytes.
    sqlx::query("UPDATE api_cloud_workspaces SET request_config=jsonb_set(request_config,'{cloudMailboxLifecycleDelivered}','{\"action\":\"archive\",\"destructionFence\":1}') WHERE workspace_id='ws-bob'").execute(&pool).await.unwrap();
    let invalidated = control
        .command_lease("ws-bob", credential, &changed, now + 34500)
        .await
        .unwrap();
    assert!(invalidated.leases.is_empty());
    assert_eq!(
        request(&app, &format!("{path}/command-wake"), "bob", None)
            .await
            .1["state"],
        "cancelled"
    );
    assert_eq!(
        runtime_post(&app, path, "bob", &envelope("stale-fence", "session-wake"))
            .await
            .0,
        StatusCode::CONFLICT
    );
    // Archived reads do not wake compute; admission and execution are both held.
    sqlx::query("UPDATE api_cloud_workspaces SET state='archived',desired_state='archived' WHERE workspace_id='ws-bob'").execute(&pool).await.unwrap();

    let archived_key = request(&app, "/v1/cloud/workspaces/ws-bob/data-key", "bob", None).await;
    assert_eq!(archived_key.0, StatusCode::OK);
    assert_eq!(archived_key.1["encodedKey"], transcript.as_str());
    assert_eq!(archived_key.1["mailboxEnabled"], false);
    assert_eq!(
        runtime_post(&app, path, "bob", &envelope("held", "session-a"))
            .await
            .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        control
            .command_lease("ws-bob", credential, &changed, now + 35000)
            .await
            .err()
            .unwrap()
            .code,
        "cloud_workspace_runtime_not_ready"
    );
    assert_eq!(
        request(&app, &format!("{path}/command-a"), "bob", None)
            .await
            .1["state"],
        "applied"
    );
    sqlx::query(
        "UPDATE api_cloud_workspaces SET desired_state='deleted' WHERE workspace_id='ws-bob'",
    )
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        control
            .command_ack("ws-bob", credential, &ack, now + 36000)
            .await
            .unwrap_err()
            .code,
        "workspace_runtime_fenced"
    );
    pool.close().await;
    sqlx::raw_sql(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .unwrap();
    admin.close().await;
}

#[tokio::test]
#[ignore = "requires isolated PostgreSQL"]
async fn authenticated_creation_is_atomic_idempotent_and_honors_existing_billing_capacity() {
    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD as B64};
    use ed25519_dalek::SigningKey;
    use zuse_cloud::{crypto::MasterKey, runtime::RuntimeControl, tickets::TicketSigner};
    let (admin, pool, schema) = fixture().await;
    sqlx::raw_sql(include_str!("fixtures/creation.sql"))
        .execute(&pool)
        .await
        .unwrap();
    let now = chrono::Utc::now().timestamp_millis();
    sqlx::query("INSERT INTO api_cloud_billing_periods(period_id,account_id,status,period_start,period_end,included_provider_cost_micros,markup_basis_points,overage_cap_micros,provider_subscription_id) VALUES('original-period','user-bob','active',$1,$2,1000,500,500,'original-subscription')").bind(now-1000).bind(now+86_400_000).execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO api_cloud_billing_ledger VALUES('original-cost','original-period','provider-cost',900)").execute(&pool).await.unwrap();
    let signing = SigningKey::from_bytes(&[26; 32]);
    let jwk = json!({"kty":"OKP","crv":"Ed25519","d":B64.encode([26;32]),"x":B64.encode(signing.verifying_key().as_bytes())});
    let key = B64.encode([27; 32]);
    let master = MasterKey::parse(&key).unwrap();
    let control = Arc::new(
        RuntimeControl::from_pool(
            pool.clone(),
            MasterKey::parse(&key).unwrap(),
            TicketSigner::from_jwk(&jwk.to_string(), "https://api-rust.zuse.sh").unwrap(),
        )
        .with_creation_accounts(vec!["user-bob".into(), "user-alice".into()]),
    );
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
    let mut body = json!({"projectId":"create-project","providerId":"boxd","baseRef":"main","branch":"cloud/created","agent":"codex","model":"fixture","firstMessage":"Original retained prompt","idempotencyKey":"create-once"});
    let (_, context) = request(&app, "/v1/cloud/context", "bob", None).await;
    assert_eq!(context["projects"][0]["supportsModelConfiguration"], false);
    let catalog: zuse_protocol::cloud_catalog::CloudAgentCatalog =
        serde_json::from_value(context["agentCatalog"].clone()).unwrap();
    assert_eq!(catalog.providers.len(), 1);
    assert_eq!(
        catalog.providers[0].id,
        zuse_protocol::HarnessId::ClaudeCode
    );
    assert!(
        catalog.providers[0]
            .resolve_model(&catalog.providers[0].default_model)
            .is_some()
    );
    body["reasoning"] = json!("high");
    body["modelOptions"] = json!({"contextWindow":"1m","fastMode":"on"});
    let endpoint = "/v1/cloud/workspaces";
    assert_eq!(
        runtime_post(&app, endpoint, "bob", &body).await.1["error"],
        "cloud_model_configuration_runtime_update_required"
    );
    let allocations: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM zuse_rust_allocations")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        allocations, 0,
        "Unsupported image must not allocate compute"
    );
    sqlx::query("UPDATE api_cloud_project_builds SET settings=settings||'{\"rustLaunchModelConfigVersion\":1}'::jsonb WHERE build_id='rust-build'").execute(&pool).await.unwrap();
    let (_, context) = request(&app, "/v1/cloud/context", "bob", None).await;
    assert_eq!(context["projects"][0]["supportsModelConfiguration"], true);
    assert_eq!(
        runtime_post(&app, endpoint, "", &body).await.0,
        StatusCode::UNAUTHORIZED
    );
    let (status, created) = runtime_post(&app, endpoint, "bob", &body).await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let id = created["workspace"]["workspaceId"].as_str().unwrap();
    assert_eq!(created["workspace"]["branch"], "cloud/created");
    let mut retries = tokio::task::JoinSet::new();
    for _ in 0..8 {
        let app = app.clone();
        let body = body.clone();
        retries
            .spawn(async move { runtime_post(&app, "/v1/cloud/workspaces", "bob", &body).await });
    }
    while let Some(reply) = retries.join_next().await {
        let (status, reply) = reply.unwrap();
        assert_eq!(status, StatusCode::OK);
        assert_eq!(reply, created);
    }
    let intent:(String,String,String)=sqlx::query_as("SELECT ciphertext,command_id,turn_id FROM api_cloud_workspace_launch_intents WHERE workspace_id=$1").bind(id).fetch_one(&pool).await.unwrap();
    let plaintext = master
        .open(
            format!("zuse-cloud-workspace-launch-intent-v1\nuser-bob\n{id}").as_bytes(),
            &intent.0,
        )
        .unwrap();
    let decoded: Value = serde_json::from_slice(&plaintext).unwrap();
    assert_eq!(decoded["firstMessage"], body["firstMessage"]);
    assert_eq!(decoded["commandId"], intent.1);
    assert_eq!(decoded["turnId"], intent.2);
    assert_eq!(decoded["reasoning"], body["reasoning"]);
    assert_eq!(decoded["modelOptions"], body["modelOptions"]);
    let counts:(i64,i64,i64)=sqlx::query_as("SELECT (SELECT COUNT(*) FROM zuse_rust_allocations WHERE workspace_id=$1),(SELECT COUNT(*) FROM zuse_rust_jobs WHERE resource_id=$1),(SELECT COUNT(*) FROM zuse_rust_control_owners WHERE resource_id=$1 AND generation=1 AND backend='rust')").bind(id).fetch_one(&pool).await.unwrap();
    assert_eq!(counts, (1, 1, 1));
    let settings: Value =
        sqlx::query_scalar("SELECT request_config FROM api_cloud_workspaces WHERE workspace_id=$1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(settings["runtimeMode"], "approval-required");
    assert_eq!(settings["permissions"], json!([]));
    assert!(!created.to_string().contains("ciphertext"));
    assert!(!created.to_string().contains("boot_token"));
    assert!(!created.to_string().contains("rustCreateRequestDigest"));
    let mut changed = body.clone();
    changed["firstMessage"] = json!("Changed on retry");
    assert_eq!(
        runtime_post(&app, endpoint, "bob", &changed).await.1["error"],
        "cloud_workspace_idempotency_conflict"
    );
    changed = body.clone();
    changed["idempotencyKey"] = json!("branch-conflict");
    assert_eq!(
        runtime_post(&app, endpoint, "bob", &changed).await.1["error"],
        "cloud_branch_in_use"
    );
    changed["branch"] = json!("cloud/other");
    changed["projectId"] = json!("private-project");
    assert_eq!(
        runtime_post(&app, endpoint, "bob", &changed).await.1["error"],
        "cloud_project_not_found"
    );
    changed["projectId"] = body["projectId"].clone();
    sqlx::query("INSERT INTO api_cloud_billing_reservations VALUES('original-period','other-runtime',600,$1)").bind(now+60_000).execute(&pool).await.unwrap();
    assert_eq!(
        runtime_post(&app, endpoint, "bob", &changed).await.1["error"],
        "cloud_billing_hold"
    );
    sqlx::query("UPDATE api_cloud_billing_reservations SET expires_at=$1")
        .bind(now - 1)
        .execute(&pool)
        .await
        .unwrap();
    // A failed durable allocation enqueue must roll back the complete accepted request.
    sqlx::raw_sql("CREATE FUNCTION fail_allocation_job() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'enqueue failed fixture'; END $$; CREATE TRIGGER fail_allocation BEFORE INSERT ON zuse_rust_jobs FOR EACH ROW EXECUTE FUNCTION fail_allocation_job();").execute(&pool).await.unwrap();
    assert_eq!(
        runtime_post(&app, endpoint, "bob", &changed).await.0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    let absent:bool=sqlx::query_scalar("SELECT NOT EXISTS(SELECT 1 FROM api_cloud_workspaces WHERE account_id='user-bob' AND idempotency_key='branch-conflict')").fetch_one(&pool).await.unwrap();
    assert!(absent);
    let counts:(i64,i64,i64,i64)=sqlx::query_as("SELECT (SELECT COUNT(*) FROM zuse_rust_allocations),(SELECT COUNT(*) FROM zuse_rust_control_owners),(SELECT COUNT(*) FROM api_cloud_workspace_launch_intents),(SELECT COUNT(*) FROM zuse_rust_jobs)").fetch_one(&pool).await.unwrap();
    assert_eq!(counts, (1, 1, 1, 1));
    sqlx::raw_sql(
        "DROP TRIGGER fail_allocation ON zuse_rust_jobs; DROP FUNCTION fail_allocation_job();",
    )
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        runtime_post(&app, endpoint, "bob", &changed).await.0,
        StatusCode::CREATED
    );
    sqlx::query("UPDATE api_cloud_billing_periods SET status='grace'")
        .execute(&pool)
        .await
        .unwrap();
    struct NoProviderEffects;
    #[async_trait]
    impl zuse_cloud::runtime::allocation::AllocationProvider for NoProviderEffects {
        async fn find(
            &self,
            _: &str,
        ) -> Result<Option<zuse_cloud::providers::Machine>, zuse_cloud::providers::ProviderError>
        {
            panic!("Billing hold must fence provider inspection");
        }
        async fn create(
            &self,
            _: &str,
            _: &str,
        ) -> Result<zuse_cloud::providers::Machine, zuse_cloud::providers::ProviderError> {
            panic!("Billing hold must fence provider allocation");
        }
        async fn launch(
            &self,
            _: &str,
            _: &zuse_cloud::runtime::allocation::RuntimeLaunch,
        ) -> Result<(), zuse_cloud::providers::ProviderError> {
            panic!("Billing hold must fence provider launch");
        }
    }
    assert_eq!(
        control
            .allocate_once(&NoProviderEffects, chrono::Utc::now().timestamp_millis())
            .await
            .unwrap_err()
            .code,
        "cloud_billing_hold"
    );
    changed["idempotencyKey"] = json!("held");
    changed["branch"] = json!("cloud/held");
    assert_eq!(
        runtime_post(&app, endpoint, "bob", &changed).await.1["error"],
        "cloud_billing_hold"
    );
    sqlx::query(
        "UPDATE api_entitlements SET status='ended',paid_through=$1 WHERE account_id='user-alice'",
    )
    .bind(now - 1)
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        runtime_post(&app, endpoint, "alice", &changed).await.1["error"],
        "cloud_entitlement_required"
    );
    let original:(String,i64)=sqlx::query_as("SELECT provider_subscription_id,(SELECT COUNT(*) FROM api_cloud_billing_ledger) FROM api_cloud_billing_periods WHERE period_id='original-period'").fetch_one(&pool).await.unwrap();
    assert_eq!(original, ("original-subscription".into(), 1));
    pool.close().await;
    sqlx::raw_sql(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .unwrap();
    admin.close().await;
}

#[derive(Default)]
struct ProjectionObjects {
    bytes: std::sync::Mutex<std::collections::HashMap<String, Vec<u8>>>,
    gets: std::sync::atomic::AtomicUsize,
    attempts: std::sync::Mutex<Vec<(String, Vec<u8>)>>,
    fail: std::sync::atomic::AtomicBool,
}
#[async_trait]
impl zuse_cloud::objects::Objects for ProjectionObjects {
    fn supports_purge(&self) -> bool {
        true
    }
    async fn purge_prefix(&self, prefix: &str) -> Result<bool, ApiError> {
        if self.fail.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(ApiError::unavailable("cloud_object_purge_failed"));
        }
        let mut map = self.bytes.lock().unwrap();
        let keys: Vec<String> = map
            .keys()
            .filter(|k| k.starts_with(prefix))
            .take(100)
            .cloned()
            .collect();
        if keys.is_empty() {
            return Ok(true);
        }
        for key in keys {
            map.remove(&key);
        }
        Ok(false)
    }
    async fn get(&self, key: &str) -> Result<Option<Vec<u8>>, ApiError> {
        self.gets.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(self.bytes.lock().unwrap().get(key).cloned())
    }
    async fn put(&self, key: &str, bytes: &[u8]) -> Result<(), ApiError> {
        self.attempts
            .lock()
            .unwrap()
            .push((key.into(), bytes.to_vec()));
        if self.fail.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(ApiError::unavailable("cloud_transcript_store_unavailable"));
        }
        let mut objects = self.bytes.lock().unwrap();
        if let Some(old) = objects.get(key) {
            if old != bytes {
                return Err(ApiError::conflict("cloud_transcript_object_collision"));
            }
        }
        objects.insert(key.into(), bytes.to_vec());
        Ok(())
    }
}
fn decrypt_projection(access: &Value) -> Value {
    use std::io::Write;
    let mut child = std::process::Command::new("node")
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/transcript-crypto.mjs"
        ))
        .arg("--decrypt")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(
            json!({"workspaceId":"ws-bob","sessionId":"session-bob","access":access})
                .to_string()
                .as_bytes(),
        )
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    serde_json::from_slice(&output.stdout).unwrap()
}
#[tokio::test]
#[ignore = "requires isolated localhost PostgreSQL and Node"]
async fn encrypted_transcript_publication_and_archived_reads_preserve_original_contracts_without_compute()
 {
    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD as B64};
    use ed25519_dalek::SigningKey;
    use sha2::{Digest, Sha256};
    use zuse_cloud::{crypto::MasterKey, runtime::RuntimeControl, tickets::TicketSigner};
    let (admin, pool, schema) = fixture().await;
    sqlx::raw_sql("CREATE TABLE api_cloud_transcript_checkpoints (workspace_id text NOT NULL,session_id text NOT NULL,runtime_generation bigint NOT NULL,stream_epoch text NOT NULL,stream_version bigint NOT NULL,object_key text NOT NULL,ciphertext_sha256 text NOT NULL,ciphertext_bytes bigint NOT NULL,created_at bigint NOT NULL,PRIMARY KEY(workspace_id,session_id))").execute(&pool).await.unwrap();
    sqlx::raw_sql(include_str!("../migrations/0011_transcript_heads.sql"))
        .execute(&pool)
        .await
        .unwrap();
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/transcript-crypto.json")).unwrap();
    let objects = Arc::new(ProjectionObjects::default());
    let now = chrono::Utc::now().timestamp_millis();
    let config = json!({"runtimeGeneration":2,"gatewayEpoch":3,"runtimeRpcProtocol":"zuse-rpc-v1","runtimeCredentialExpiresAtMs":now+60_000,"runtimeCredentialKeyThumbprint":"credential-key","runtimeSigningKeyThumbprint":"signing-key","runtimeBootstrapReceipt":{"workspaceId":"ws-bob","generation":2,"gatewayEpoch":3},"rustRuntimeStorage":{"format":"zuse-rust-engine-v1","storageIncarnationId":"original-storage"},"rustStorageEnrollment":{"phase":"committed","generation":2,"sourceSandboxId":"machine-bob","storageIncarnationId":"original-storage","credentialKeyThumbprint":"credential-key","signingKeyThumbprint":"signing-key"}});
    sqlx::query("INSERT INTO zuse_rust_control_owners VALUES('workspace','ws-bob','rust',2,$1)")
        .bind(now)
        .execute(&pool)
        .await
        .unwrap();
    let bearer = "fixture-transcript-runtime";
    let hash: String = Sha256::digest(bearer.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    sqlx::query("UPDATE api_cloud_workspaces SET state='archiving',desired_state='archived',request_config=$1,runtime_credential_hash=$2,wrapped_transcript_key=$3 WHERE workspace_id='ws-bob'").bind(config).bind(hash).bind(fixture["wrappedKey"].as_str()).execute(&pool).await.unwrap();
    let signing = SigningKey::from_bytes(&[30; 32]);
    let jwk = json!({"kty":"OKP","crv":"Ed25519","d":B64.encode([30;32]),"x":B64.encode(signing.verifying_key().as_bytes())});
    let control = Arc::new(
        RuntimeControl::from_pool(
            pool.clone(),
            MasterKey::parse(fixture["masterKey"].as_str().unwrap()).unwrap(),
            TicketSigner::from_jwk(&jwk.to_string(), "https://api-rust.zuse.sh").unwrap(),
        )
        .with_transcript_objects(objects.clone()),
    );
    let state = AppState {
        store: Store::from_pool(pool.clone()).with_transcripts(
            objects.clone(),
            MasterKey::parse(fixture["masterKey"].as_str().unwrap()).unwrap(),
        ),
        identity: Arc::new(TestIdentity),
        organizations_enabled: false,
        organization_ids: Arc::new(vec![]),
        runtime: Some(control.clone()),
        gateway: Arc::new(zuse_cloud::gateway::Hub::default()),
    };
    let app = router(state.clone(), vec![]);
    let upload = "/v1/cloud/workspaces/ws-bob/runtime/transcript-checkpoint";
    assert_eq!(
        runtime_post(&app, upload, "wrong", &fixture["head1"])
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    let first = runtime_post(&app, upload, bearer, &fixture["head1"]).await;
    assert_eq!(first.0, StatusCode::OK, "{}", first.1);
    assert_eq!(first.1["applied"], true);
    assert_eq!(first.1["archiveRequested"], true);
    assert_eq!(
        runtime_post(&app, upload, bearer, &fixture["head1"])
            .await
            .1["applied"],
        false
    );
    objects
        .fail
        .store(true, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(
        runtime_post(&app, upload, bearer, &fixture["head2"])
            .await
            .0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    let version: i64 =
        sqlx::query_scalar("SELECT stream_version FROM api_cloud_transcript_checkpoints")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(version, 1);
    objects
        .fail
        .store(false, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(
        runtime_post(&app, upload, bearer, &fixture["head2"])
            .await
            .1["applied"],
        true
    );
    assert_eq!(
        runtime_post(&app, upload, bearer, &fixture["head1"])
            .await
            .1["applied"],
        false
    );
    let page_upload = "/v1/cloud/workspaces/ws-bob/runtime/transcript-message-page";
    assert_eq!(
        runtime_post(&app, page_upload, bearer, &fixture["page"])
            .await
            .1,
        json!({"applied":true})
    );
    let mut stale = fixture["page"].clone();
    stale["cursor"]["version"] = json!(1);
    assert_eq!(
        runtime_post(&app, page_upload, bearer, &stale).await.0,
        StatusCode::CONFLICT
    );
    let mut changed = fixture["head2"].clone();
    changed["ciphertext"] = json!("changed bytes");
    changed["ciphertextSha256"] = json!(zuse_cloud::transcript::digest(b"changed bytes"));
    assert_eq!(
        runtime_post(&app, upload, bearer, &changed).await.0,
        StatusCode::CONFLICT
    );
    let mut invalid = fixture["head2"].clone();
    invalid["ciphertextSha256"] = json!("wrong");
    assert_eq!(
        runtime_post(&app, upload, bearer, &invalid).await.0,
        StatusCode::BAD_REQUEST
    );
    // Reads keep working after writer ownership is removed, as for unmigrated legacy rows.
    sqlx::query("DELETE FROM zuse_rust_control_owners WHERE resource_id='ws-bob'")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE api_cloud_workspaces SET state='archived',runtime_state='offline' WHERE workspace_id='ws-bob'").execute(&pool).await.unwrap();
    let before: Value = sqlx::query_scalar(
        "SELECT to_jsonb(w) FROM api_cloud_workspaces w WHERE workspace_id='ws-bob'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let read_app = router(
        AppState {
            runtime: None,
            ..state
        },
        vec![],
    );
    let read = "/v1/cloud/workspaces/ws-bob/sessions/session-bob/transcript-checkpoint";
    assert_eq!(
        request(&read_app, read, "alice", None).await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        request(&read_app, read, "", None).await.0,
        StatusCode::UNAUTHORIZED
    );
    let (status, body) = request(&read_app, read, "bob", None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        objects.gets.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "Opening screen must come from its Postgres cursor row without object reads"
    );
    assert_eq!(
        body["checkpoint"]["ciphertext"],
        fixture["head2"]["ciphertext"]
    );
    assert_eq!(
        body["checkpoint"]["transcriptKey"],
        fixture["transcriptKey"]
    );
    let payload = decrypt_projection(&body["checkpoint"]);
    assert_eq!(
        payload["projection"]["messages"][0]["content"]["text"],
        "Latest preserved user text"
    );
    let gets = objects.gets.load(std::sync::atomic::Ordering::SeqCst);
    let caught_up = format!("{read}?epoch=original%2Fepoch%20%21%C3%A9&version=2.0");
    assert_eq!(
        request(&read_app, &caught_up, "bob", None).await.1,
        json!({"checkpoint":null})
    );
    assert_eq!(gets, objects.gets.load(std::sync::atomic::Ordering::SeqCst));
    let page = "/v1/cloud/workspaces/ws-bob/sessions/session-bob/transcript-message-page?epoch=original%2Fepoch%20%21%C3%A9&version=2&beforeSequence=10";
    let (status, body) = request(&read_app, page, "bob", None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        decrypt_projection(&body["page"])["messages"][0]["content"]["text"],
        "Older preserved user text"
    );
    assert_eq!(
        request(
            &read_app,
            &page.replace("beforeSequence=10", "beforeSequence=9"),
            "bob",
            None
        )
        .await
        .1,
        json!({"page":null})
    );
    // Corrupted hot data fails integrity checks; a stale cache tag falls back to
    // the authoritative object's current cursor rather than returning old text.
    let saved_hot: Value =
        sqlx::query_scalar("SELECT rust_hot_head FROM api_cloud_transcript_checkpoints")
            .fetch_one(&pool)
            .await
            .unwrap();
    sqlx::query("UPDATE api_cloud_transcript_checkpoints SET rust_hot_head=jsonb_set(rust_hot_head,'{ciphertext}','\"tampered\"')").execute(&pool).await.unwrap();
    assert_eq!(
        request(&read_app, read, "bob", None).await.1["error"],
        "cloud_transcript_integrity_failed"
    );
    sqlx::query(
        "UPDATE api_cloud_transcript_checkpoints SET rust_hot_head=jsonb_set($1,'{version}','1')",
    )
    .bind(saved_hot)
    .execute(&pool)
    .await
    .unwrap();
    let gets = objects.gets.load(std::sync::atomic::Ordering::SeqCst);
    assert_eq!(
        request(&read_app, read, "bob", None).await.0,
        StatusCode::OK
    );
    assert_eq!(
        objects.gets.load(std::sync::atomic::Ordering::SeqCst),
        gets + 1
    );
    let key = objects
        .bytes
        .lock()
        .unwrap()
        .keys()
        .find(|k| k.ends_with("/2.json"))
        .cloned()
        .unwrap();
    objects
        .bytes
        .lock()
        .unwrap()
        .insert(key, b"corrupt ciphertext".to_vec());
    assert_eq!(
        request(&read_app, read, "bob", None).await.1["error"],
        "cloud_transcript_integrity_failed"
    );
    let after: Value = sqlx::query_scalar(
        "SELECT to_jsonb(w) FROM api_cloud_workspaces w WHERE workspace_id='ws-bob'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(before, after);
    let jobs: i64 = sqlx::query_scalar("SELECT count(*) FROM zuse_rust_jobs")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(jobs, 0);
    sqlx::query(
        "UPDATE api_cloud_workspaces SET desired_state='deleted' WHERE workspace_id='ws-bob'",
    )
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        request(&read_app, read, "bob", None).await.0,
        StatusCode::NOT_FOUND
    );
    pool.close().await;
    sqlx::raw_sql(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .unwrap();
    admin.close().await;
}

#[tokio::test]
#[ignore = "requires explicit isolated localhost PostgreSQL"]
async fn cloud_billing_summary_usage_and_owned_cap_workflow() {
    let (admin, pool, schema) = fixture().await;
    sqlx::raw_sql(include_str!("fixtures/billing.sql"))
        .execute(&pool)
        .await
        .unwrap();
    let now = chrono::Utc::now().timestamp_millis();
    for (id, owner) in [
        ("p-alice", "user-alice"),
        ("p-team", "organization:org-team"),
    ] {
        sqlx::query("INSERT INTO api_cloud_billing_periods VALUES($1,$2,'stripe','sub-preserved','active','USD',$3,$4,40000000,35000000,500,25000000,$3)").bind(id).bind(owner).bind(now-1000).bind(now+3600000).execute(&pool).await.unwrap();
    }
    sqlx::raw_sql("INSERT INTO api_cloud_billing_ledger VALUES('cost','p-alice','provider-cost',40000000),('charge','p-alice','overage-charge',5250000); INSERT INTO api_cloud_billing_meter_reconciliations VALUES('p-alice','stripe',123),('p-alice','other',999); INSERT INTO api_cloud_billing_usage VALUES('z-entry','p-alice','workspace','cloud-one','boxd','execution-one',10,20,2,4096,36000000,'confirmed',50),('y-entry','p-alice','snapshot','snap-one','boxd',NULL,10,20,0,0,4000000,'corrected',60);").execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO api_cloud_billing_reservations VALUES('p-alice','workspace','active','boxd',10,20,2,4096,2000000,$1),('p-alice','workspace','expired','boxd',10,20,2,4096,99999999,0)").bind(now+3600000).execute(&pool).await.unwrap();
    let store = Store::from_pool(pool.clone()).with_billing_control(
        zuse_cloud::billing::BillingControl::from_pool(
            pool.clone(),
            vec!["user-alice".into(), "organization:org-team".into()],
        ),
    );
    let state = AppState {
        store,
        identity: Arc::new(TestIdentity),
        organizations_enabled: true,
        organization_ids: Arc::new(vec!["org-team".into()]),
        runtime: None,
        gateway: Arc::new(zuse_cloud::gateway::Hub::default()),
    };
    let app = router(state.clone(), vec![]);
    let path = "/v1/cloud/billing/summary";
    assert_eq!(
        request(&app, path, "", None).await.0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        request(&app, path, "member", Some("organization:org-team"))
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        request(&app, path, "billing", Some("organization:org-team"))
            .await
            .1["providerCostMicros"],
        0
    );
    assert_eq!(
        request(&app, path, "bob", None).await.1["error"],
        "cloud_billing_subscription_required"
    );
    let (status, summary) = request(&app, path, "alice", None).await;
    assert_eq!(status, StatusCode::OK, "{summary}");
    assert_eq!(summary["providerCostMicros"], 42000000);
    assert_eq!(summary["storageCostMicros"], 4000000);
    assert_eq!(summary["overageChargeMicros"], 7350000);
    assert_eq!(summary["currentInvoiceEstimateMicros"], 47350000);
    assert_eq!(summary["usageProvisional"], true);
    assert_eq!(summary["lastProviderReconciledAt"], 60);
    assert_eq!(summary["lastBillingReconciledAt"], 123);
    let (status, page) = request(&app, "/v1/cloud/billing/usage?limit=2", "alice", None).await;
    assert_eq!(status, StatusCode::OK, "{page}");
    assert_eq!(page["items"].as_array().unwrap().len(), 2);
    assert_eq!(page["items"][1]["resourceKind"], "other");
    assert_eq!(page["items"][1]["usageKind"], "snapshot-storage");
    assert_eq!(page["nextCursor"], "y-entry");
    let next = request(
        &app,
        "/v1/cloud/billing/usage?cursor=y-entry&limit=2",
        "alice",
        None,
    )
    .await
    .1;
    assert_eq!(next["items"].as_array().unwrap().len(), 1);
    assert_eq!(next["items"][0]["entryId"], "reservation:workspace:active");
    assert_eq!(next["items"][0]["status"], "provisional");
    assert!(next.get("nextCursor").is_none());
    let cap = "/v1/cloud/billing/cap";
    let body = json!({"overageCapMicros":6000000,"idempotencyKey":"first"});
    assert_eq!(
        runtime_post(&app, cap, "alice", &body).await.1["error"],
        "cloud_billing_owner_not_rust"
    );
    sqlx::query("INSERT INTO zuse_rust_control_owners VALUES('billing','user-alice','rust',1,$1)")
        .bind(now)
        .execute(&pool)
        .await
        .unwrap();
    let (status, updated) = runtime_post(&app, cap, "alice", &body).await;
    assert_eq!(status, StatusCode::OK, "{updated}");
    assert_eq!(updated["overageCapMicros"], 6000000);
    assert_eq!(updated["overageChargeMicros"], 6000000);
    assert_eq!(updated["status"], "billing-hold");
    assert_eq!(
        runtime_post(
            &app,
            cap,
            "alice",
            &json!({"overageCapMicros":8000000,"idempotencyKey":"second"})
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        runtime_post(&app, cap, "alice", &body).await.1["overageCapMicros"],
        8000000,
        "Old retries must not rewind the cap"
    );
    assert_eq!(
        runtime_post(
            &app,
            cap,
            "alice",
            &json!({"overageCapMicros":9000000,"idempotencyKey":"first"})
        )
        .await
        .1["error"],
        "billing_cap_idempotency_conflict"
    );
    assert_eq!(
        runtime_post(
            &app,
            cap,
            "alice",
            &json!({"overageCapMicros":5000000,"idempotencyKey":"below"})
        )
        .await
        .1["error"],
        "billing_cap_below_incurred"
    );
    for value in [json!(-1), json!(1.5), json!(9007199254740992i64)] {
        assert_eq!(
            runtime_post(
                &app,
                cap,
                "alice",
                &json!({"overageCapMicros":value,"idempotencyKey":"invalid"})
            )
            .await
            .0,
            StatusCode::BAD_REQUEST
        );
    }
    let a = json!({"overageCapMicros":10000000,"idempotencyKey":"concurrent"});
    let b = json!({"overageCapMicros":11000000,"idempotencyKey":"concurrent"});
    let (a, b) = tokio::join!(
        runtime_post(&app, cap, "alice", &a),
        runtime_post(&app, cap, "alice", &b)
    );
    assert!(
        (a.0 == StatusCode::OK && b.0 == StatusCode::CONFLICT)
            || (b.0 == StatusCode::OK && a.0 == StatusCode::CONFLICT)
    );
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM api_cloud_billing_ledger")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 2);
    let row: Value = sqlx::query_scalar(
        "SELECT to_jsonb(p) FROM api_cloud_billing_periods p WHERE period_id='p-alice'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(row["provider_subscription_id"], "sub-preserved");
    assert_eq!(row["billing_provider"], "stripe");
    if let Ok(path) = std::env::var("ZUSE_CLOUD_BILLING_UI_FIXTURE_FILE") {
        sqlx::raw_sql("INSERT INTO api_cloud_billing_usage SELECT 'u-extra-'||lpad(n::text,3,'0'),'p-alice','workspace','history-'||n,'boxd',NULL,10,20,2,4096,0,'confirmed',60 FROM generate_series(1,52) n").execute(&pool).await.unwrap();
        assert_eq!(
            runtime_post(
                &app,
                cap,
                "alice",
                &json!({"overageCapMicros":25000000,"idempotencyKey":"ui-start"})
            )
            .await
            .0,
            StatusCode::OK
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(axum::serve(listener, app.clone()).into_future());
        let file = std::path::Path::new(&path);
        std::fs::write(file, origin).unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(900), async {
            while !file.with_extension("done").exists() {
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
        })
        .await
        .expect("Billing UI acceptance timed out");
        let cap: i64 = sqlx::query_scalar(
            "SELECT overage_cap_micros FROM api_cloud_billing_periods WHERE period_id='p-alice'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(cap, 31000000);
        server.abort();
    }
    let legacy = router(
        AppState {
            store: Store::from_pool(pool.clone()),
            ..state
        },
        vec![],
    );
    assert_eq!(
        runtime_post(&legacy, cap, "alice", &body).await.1["error"],
        "cloud_billing_writes_disabled"
    );
    pool.close().await;
    sqlx::raw_sql(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .unwrap();
    admin.close().await;
}

#[tokio::test]
#[ignore = "requires explicit isolated localhost PostgreSQL"]
async fn billing_portal_preserves_provider_customer_and_owner() {
    use zuse_cloud::billing::{BillingControl, PaymentProviders, PortalProvider};
    #[derive(Clone)]
    struct Provider(
        &'static str,
        Arc<std::sync::Mutex<Vec<(String, Option<String>)>>>,
    );
    #[async_trait]
    impl PortalProvider for Provider {
        fn provider_id(&self) -> &'static str {
            self.0
        }
        async fn portal(&self, owner: &str, customer: Option<&str>) -> Result<String, ApiError> {
            self.1
                .lock()
                .unwrap()
                .push((owner.into(), customer.map(str::to_owned)));
            Ok(format!("https://{}.example.test/portal", self.0))
        }
    }
    let (admin, pool, schema) = fixture().await;
    sqlx::raw_sql("CREATE TABLE api_stripe_customers(account_id text PRIMARY KEY, customer_id text UNIQUE); INSERT INTO api_stripe_customers VALUES('user-alice','cus_original'),('organization:org-team','cus_org'); INSERT INTO zuse_rust_control_owners(resource_kind,resource_id,backend,generation,updated_at) VALUES('billing','user-alice','rust',1,0),('billing','user-bob','rust',1,0),('billing','organization:org-team','rust',1,0);").execute(&pool).await.unwrap();
    let calls = Arc::new(std::sync::Mutex::new(vec![]));
    let state = AppState {
        store: Store::from_pool(pool.clone()).with_billing_control(
            BillingControl::from_pool(
                pool.clone(),
                vec![
                    "user-alice".into(),
                    "user-bob".into(),
                    "organization:org-team".into(),
                ],
            )
            .with_payment_providers(
                PaymentProviders::new(
                    "stripe".into(),
                    vec![Arc::new(Provider("stripe", calls.clone()))],
                )
                .unwrap(),
            ),
        ),
        identity: Arc::new(TestIdentity),
        organizations_enabled: true,
        organization_ids: Arc::new(vec!["org-team".into()]),
        runtime: None,
        gateway: Arc::new(zuse_cloud::gateway::Hub::default()),
    };
    let app = router(state.clone(), vec![]);
    let path = "/v1/billing/portal";
    assert_eq!(
        runtime_post(&app, path, "", &json!({})).await.0,
        StatusCode::UNAUTHORIZED
    );
    let org_request = |token: &str| {
        Request::builder()
            .method("POST")
            .uri(path)
            .header("authorization", format!("Bearer {token}"))
            .header("x-zuse-workspace", "organization:org-team")
            .body(Body::empty())
            .unwrap()
    };
    assert_eq!(
        app.clone()
            .oneshot(org_request("member"))
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );
    assert!(calls.lock().unwrap().is_empty());
    let response = runtime_post(&app, path, "alice", &json!({})).await;
    assert_eq!(response.0, StatusCode::OK);
    assert_eq!(
        response.1["portalUrl"],
        "https://stripe.example.test/portal"
    );
    let response = runtime_post(&app, path, "bob", &json!({})).await;
    assert_eq!(response.0, StatusCode::OK);
    assert_eq!(
        response.1["portalUrl"],
        "https://stripe.example.test/portal"
    );
    assert_eq!(
        app.clone()
            .oneshot(org_request("billing"))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        *calls.lock().unwrap(),
        vec![
            ("user-alice".into(), Some("cus_original".into())),
            ("user-bob".into(), None),
            ("organization:org-team".into(), Some("cus_org".into()))
        ]
    );
    sqlx::query(
        "UPDATE zuse_rust_control_owners SET backend='legacy' WHERE resource_id='user-alice'",
    )
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        runtime_post(&app, path, "alice", &json!({})).await.1["error"],
        "cloud_billing_owner_not_rust"
    );
    sqlx::raw_sql("UPDATE zuse_rust_control_owners SET backend='rust' WHERE resource_id='user-alice'; INSERT INTO api_entitlements VALUES('conflict','user-alice','cloud-workspace','cloud-workspace-standard-v1','other','sub-other','active',9999999999999,NULL,100,100)").execute(&pool).await.unwrap();
    assert_eq!(
        runtime_post(&app, path, "alice", &json!({})).await.1["error"],
        "billing_provider_ambiguous"
    );
    assert_eq!(calls.lock().unwrap().len(), 3);
    sqlx::query("UPDATE api_entitlements SET status='ended' WHERE entitlement_id='conflict'")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        runtime_post(&app, path, "alice", &json!({})).await.0,
        StatusCode::OK
    );
    let unchanged: String = sqlx::query_scalar(
        "SELECT provider_subscription_id FROM api_entitlements WHERE entitlement_id='ent-alice'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(unchanged, "sub-preserved");
    let legacy = router(
        AppState {
            store: Store::from_pool(pool.clone()),
            ..state.clone()
        },
        vec![],
    );
    assert_eq!(
        runtime_post(&legacy, path, "alice", &json!({})).await.1["error"],
        "cloud_billing_writes_disabled"
    );
    if let Ok(path) = std::env::var("ZUSE_CLOUD_BILLING_UI_FIXTURE_FILE") {
        sqlx::raw_sql(include_str!("fixtures/billing.sql"))
            .execute(&pool)
            .await
            .unwrap();
        let now = chrono::Utc::now().timestamp_millis();
        sqlx::query("INSERT INTO api_cloud_billing_periods VALUES('p-alice','user-alice','stripe','sub-preserved','active','USD',$1,$2,40000000,35000000,500,25000000,$1)").bind(now-1000).bind(now+3600000).execute(&pool).await.unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(axum::serve(listener, app).into_future());
        let file = std::path::Path::new(&path);
        std::fs::write(file, origin).unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(900), async {
            while !file.with_extension("done").exists() {
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
        })
        .await
        .expect("Portal UI acceptance timed out");
        assert!(
            calls.lock().unwrap().len() >= 5,
            "UI must create a portal session through the actual API"
        );
        server.abort();
    }
    pool.close().await;
    sqlx::raw_sql(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .unwrap();
    admin.close().await;
}

#[path = "cases/launch_assets.rs"]
mod launch_assets;

#[path = "cases/startup_binding.rs"]
mod startup_binding;
