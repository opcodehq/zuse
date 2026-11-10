//! Actual HTTP/PG grant regression: retain fresh fences while eliminating duplicate reads.
use super::*;
use crate::connections::{
    ConnectionControl,
    worker::{AuthDriver, AuthJob, DeviceChallenge, VerifiedConnection},
};
use axum::{Json, Router, response::IntoResponse, routing::post};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD as B64};
use rsa::traits::PublicKeyParts;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::mpsc;
struct NeverRefresh(AtomicUsize);
#[async_trait]
impl AuthDriver for NeverRefresh {
    async fn execute(
        &self,
        _: AuthJob,
        _: mpsc::Sender<DeviceChallenge>,
    ) -> Result<VerifiedConnection, &'static str> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Err("unexpired_grant_must_not_refresh")
    }
}
async fn measured_pool(url: &str, schema: &str, count: Arc<AtomicUsize>) -> PgPool {
    let schema = schema.to_owned();
    sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .after_connect(move |c, _| {
            let schema = schema.clone();
            Box::pin(async move {
                sqlx::query(&format!("SET search_path={schema}"))
                    .execute(c)
                    .await?;
                Ok(())
            })
        })
        .before_acquire(move |_, _| {
            count.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { Ok(true) })
        })
        .connect(url)
        .await
        .unwrap()
}
#[tokio::test]
#[ignore = "requires explicit isolated localhost PostgreSQL"]
async fn unexpired_http_grant_uses_locked_selection_and_only_reads_credential_inside_refresh_gate()
{
    let url = std::env::var("ZUSE_CLOUD_TEST_DATABASE_URL").unwrap();
    assert!(matches!(
        reqwest::Url::parse(&url).unwrap().host_str(),
        Some("localhost" | "127.0.0.1" | "::1")
    ));
    let admin = PgPool::connect(&url).await.unwrap();
    let schema = format!("grant_latency_{}", uuid::Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&admin)
        .await
        .unwrap();
    let control_reads = Arc::new(AtomicUsize::new(0));
    let authority_reads = Arc::new(AtomicUsize::new(0));
    let pool = measured_pool(&url, &schema, control_reads.clone()).await;
    let authority_pool = measured_pool(&url, &schema, authority_reads.clone()).await;
    sqlx::raw_sql(include_str!("../../migrations/0001_control_plane.sql"))
        .execute(&pool)
        .await
        .unwrap();
    sqlx::raw_sql("CREATE TABLE api_cloud_workspaces(workspace_id text PRIMARY KEY,account_id text,provider_sandbox_id text,runtime_credential_hash text,state text,desired_state text,request_config jsonb);CREATE TABLE cloud_agent_connections(connection_id text PRIMARY KEY,owner_id text,agent text,method text,state text,credential_ciphertext text,version bigint,authority_epoch bigint,expires_at bigint,refresh_lease_id text,refresh_lease_expires_at bigint,provider_account_id text,account_label text,last_error_code text,updated_at bigint)").execute(&pool).await.unwrap();
    sqlx::raw_sql(include_str!(
        "../../migrations/0024_credential_delivery_journal.sql"
    ))
    .execute(&pool)
    .await
    .unwrap();
    let key = B64.encode([81; 32]);
    let master = crate::crypto::MasterKey::parse(&key).unwrap();
    let signing = ed25519_dalek::SigningKey::from_bytes(&[82; 32]);
    let jwk = json!({"kty":"OKP","crv":"Ed25519","d":B64.encode([82;32]),"x":B64.encode(signing.verifying_key().as_bytes())});
    let secret=master.seal_api("agent-connection:member:connection",&json!({"formatVersion":1,"agent":"codex","method":"device-code","accessToken":"fixture-unexpired-access","refreshToken":"fixture-renewable-must-never-be-delivered","providerAccountId":"same-vendor-account"}).to_string()).unwrap();
    let authority = Arc::new(ConnectionControl::from_pool(
        authority_pool.clone(),
        master,
        vec!["member".into()],
    ));
    let control = Arc::new(
        RuntimeControl::from_pool(
            pool.clone(),
            crate::crypto::MasterKey::parse(&key).unwrap(),
            crate::tickets::TicketSigner::from_jwk(&jwk.to_string(), "https://api.fixture.test")
                .unwrap(),
        )
        .with_credential_authority(authority),
    );
    let rsa = rsa::RsaPrivateKey::new(&mut rand::thread_rng(), 2048).unwrap();
    let public=json!({"kty":"RSA","n":B64.encode(rsa.n().to_bytes_be()),"e":B64.encode(rsa.e().to_bytes_be())}).to_string();
    let thumb = credential_thumbprint(&public).unwrap();
    let now = chrono::Utc::now().timestamp_millis();
    let config = json!({"agent":"codex","rustActorId":"member","runtimeRpcProtocol":zuse_protocol::cloud_gateway::RUST_RPC,"runtimeGeneration":1,"gatewayEpoch":1,"runtimeBootstrapReceipt":{"workspaceId":"task","generation":1,"gatewayEpoch":1},"runtimeCredentialExpiresAtMs":now+600000,"runtimeCredentialKeyThumbprint":thumb,"runtimeSigningKeyThumbprint":"signing","providerAuthMode":"broker-v1","agentConnectionId":"connection","agentConnectionOwnerId":"member","rustRuntimeStorage":{"storageIncarnationId":"disk"},"rustStorageEnrollment":{"phase":"committed","generation":1,"sourceSandboxId":"sandbox","storageIncarnationId":"disk","credentialKeyThumbprint":thumb,"signingKeyThumbprint":"signing"}});
    sqlx::query("INSERT INTO zuse_rust_control_owners VALUES('workspace','task','rust',1,$1)")
        .bind(now)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO api_cloud_workspaces VALUES('task','member','sandbox',$1,'ready','ready',$2)",
    )
    .bind(hash("runtime"))
    .bind(config)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO cloud_agent_connections VALUES('connection','member','codex','device-code','connected',$1,3,2,$2,NULL,NULL,'same-vendor-account',NULL,NULL,$3)").bind(secret).bind(now+3600000).bind(now).execute(&pool).await.unwrap();
    // Warm the SQL statement separately; this fixture retains fresh scope reads.
    control.scope_access("task", None, true).await.unwrap();
    let driver = Arc::new(NeverRefresh(AtomicUsize::new(0)));
    let server_control = control.clone();
    let server_driver = driver.clone();
    let app = Router::new().route(
        "/grant",
        post(move |Json(request): Json<ProviderGrantRequest>| {
            let control = server_control.clone();
            let driver = server_driver.clone();
            async move {
                match control
                    .provider_grant_with_driver(
                        "task",
                        "runtime",
                        CloudAuthProvider::Codex,
                        request,
                        chrono::Utc::now().timestamp_millis(),
                        Some(driver.as_ref()),
                    )
                    .await
                {
                    Ok(grant) => Json(grant).into_response(),
                    Err(error) => error.into_response(),
                }
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}/grant", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let request = ProviderGrantRequest {
        protocol_version: 1,
        request_id: uuid::Uuid::new_v4().to_string(),
        runtime_generation: 1,
        credential_public_jwk: public,
        reason: zuse_protocol::cloud_auth::GrantReason::Initial,
        previous_provider_account_id: Some("same-vendor-account".into()),
        previous_chatgpt_account_id: None,
    };
    control_reads.store(0, Ordering::SeqCst);
    authority_reads.store(0, Ordering::SeqCst);
    let start = std::time::Instant::now();
    let response = reqwest::Client::new()
        .post(&endpoint)
        .json(&request)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let grant: SealedProviderGrant = response.json().await.unwrap();
    let elapsed = start.elapsed().as_micros();
    let measured_control = control_reads.load(Ordering::SeqCst);
    let measured_authority = authority_reads.load(Ordering::SeqCst);
    assert_eq!(grant.request_id, request.request_id);
    assert_eq!(grant.authority_epoch, 2);
    assert!(
        !serde_json::to_string(&grant)
            .unwrap()
            .contains("fixture-unexpired-access")
    );
    assert!(
        !serde_json::to_string(&grant)
            .unwrap()
            .contains("fixture-renewable")
    );
    // Initial-agent fast paths must still reject malformed or conflicting maps;
    // a selected connection may never silently become the launch connection.
    for binding in [
        Value::Null,
        json!({"codex":{"ownerId":"member","connectionId":"other-connected-account"}}),
        json!({"codex":{"ownerId":"administrator","connectionId":"connection"}}),
    ] {
        sqlx::query("UPDATE api_cloud_workspaces SET request_config=jsonb_set(request_config,'{agentConnectionBindings}',$1)")
            .bind(binding).execute(&pool).await.unwrap();
        let mut conflicting = serde_json::to_value(&request).unwrap();
        conflicting["requestId"] = json!(uuid::Uuid::new_v4().to_string());
        let rejected = reqwest::Client::new()
            .post(&endpoint)
            .json(&conflicting)
            .send()
            .await
            .unwrap();
        assert!(
            !rejected.status().is_success(),
            "must not borrow the launch account"
        );
    }
    // A correctly bound initial account remains usable with the same owner.
    sqlx::query("UPDATE api_cloud_workspaces SET request_config=jsonb_set(request_config,'{agentConnectionBindings}',$1)")
        .bind(json!({"codex":{"ownerId":"member","connectionId":"connection"}}))
        .execute(&pool).await.unwrap();
    let mut explicit = serde_json::to_value(&request).unwrap();
    explicit["requestId"] = json!(uuid::Uuid::new_v4().to_string());
    let explicit_response = reqwest::Client::new()
        .post(&endpoint)
        .json(&explicit)
        .send()
        .await
        .unwrap();
    assert_eq!(explicit_response.status(), 200);
    let explicit_grant: SealedProviderGrant = explicit_response.json().await.unwrap();
    assert_eq!(explicit_grant.authority_incarnation_id, "connection");
    assert_eq!(explicit_grant.authority_epoch, 2);
    sqlx::query(
        "UPDATE api_cloud_workspaces SET request_config=request_config-'agentConnectionBindings'",
    )
    .execute(&pool)
    .await
    .unwrap();
    // A new request still sees current revocation; it never chooses another account.
    sqlx::query("UPDATE cloud_agent_connections SET state='revoked',authority_epoch=3,version=4")
        .execute(&pool)
        .await
        .unwrap();
    let revoked = reqwest::Client::new()
        .post(&endpoint)
        .json(&ProviderGrantRequest {
            request_id: uuid::Uuid::new_v4().to_string(),
            ..request
        })
        .send()
        .await
        .unwrap();
    assert!(!revoked.status().is_success());
    assert_eq!(driver.0.load(Ordering::SeqCst), 0);
    server.abort();
    let _ = server.await;
    pool.close().await;
    authority_pool.close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .unwrap();
    admin.close().await;
    println!(
        "grant_component_counts control_pool_checkouts={measured_control} authority_pool_checkouts={measured_authority} local_http_us={elapsed} vendor_refreshes=0"
    );
    assert_eq!(
        measured_authority, 4,
        "fresh pre/post delivery binding plus one credential read inside the gate and one rotation recovery read"
    );
    assert_eq!(
        measured_control, 7,
        "reuse the locked selection and batch cleanup/receipt reads on existing connections, while keeping fresh scope checks"
    );
}
