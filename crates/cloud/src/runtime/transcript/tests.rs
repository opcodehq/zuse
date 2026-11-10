use super::*;
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use tokio::{
    sync::Semaphore,
    time::{Duration, timeout},
};

struct SlowStore {
    entered: Semaphore,
    release: Semaphore,
}
#[async_trait::async_trait]
impl crate::objects::Objects for SlowStore {
    async fn get(&self, _: &str) -> Result<Option<Vec<u8>>, ApiError> {
        Ok(None)
    }
    async fn put(&self, _: &str, _: &[u8]) -> Result<(), ApiError> {
        self.entered.add_permits(1);
        self.release.acquire().await.unwrap().forget();
        Ok(())
    }
}
#[tokio::test]
#[ignore = "requires isolated localhost PostgreSQL"]
async fn slow_projection_put_does_not_block_runtime_gateway_and_fences_late_results() {
    let url = std::env::var("ZUSE_CLOUD_TEST_DATABASE_URL").unwrap();
    assert!(matches!(
        reqwest::Url::parse(&url).unwrap().host_str(),
        Some("localhost" | "127.0.0.1" | "::1")
    ));
    let admin = PgPool::connect(&url).await.unwrap();
    let schema = format!("projection_lock_{}", uuid::Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&admin)
        .await
        .unwrap();
    let scope = schema.clone();
    let pool = PgPoolOptions::new()
        .max_connections(5)
        .after_connect(move |c, _| {
            let scope = scope.clone();
            Box::pin(async move {
                sqlx::query(&format!("SET search_path={scope}"))
                    .execute(c)
                    .await?;
                Ok(())
            })
        })
        .connect(&url)
        .await
        .unwrap();
    for sql in [
        include_str!("../../../tests/fixtures/legacy-catalog.sql"),
        crate::jobs::MIGRATION,
        include_str!("../../../migrations/0002_gateway_leases.sql"),
        "CREATE TABLE api_cloud_transcript_checkpoints(workspace_id text,session_id text,runtime_generation bigint,stream_epoch text,stream_version bigint,object_key text,ciphertext_sha256 text,ciphertext_bytes bigint,created_at bigint,PRIMARY KEY(workspace_id,session_id))",
    ] {
        sqlx::raw_sql(sql).execute(&pool).await.unwrap();
    }
    let intent_table = include_str!("../../../migrations/0026_task_command_admission.sql")
        .split("CREATE TABLE IF NOT EXISTS zuse_rust_object_write_intents")
        .nth(1)
        .unwrap();
    sqlx::raw_sql(&format!(
        "CREATE TABLE IF NOT EXISTS zuse_rust_object_write_intents{intent_table}"
    ))
    .execute(&pool)
    .await
    .unwrap();
    sqlx::raw_sql(include_str!(
        "../../../migrations/0030_transcript_write_intents.sql"
    ))
    .execute(&pool)
    .await
    .unwrap();
    let now = chrono::Utc::now().timestamp_millis();
    let token = "projection-fixture-runtime";
    let config = json!({"runtimeGeneration":2,"gatewayEpoch":3,"runtimeRpcProtocol":"zuse-rpc-v1","runtimeCredentialExpiresAtMs":now+60000,
        "runtimeCredentialKeyThumbprint":"credential-key","runtimeSigningKeyThumbprint":"signing-key",
        "runtimeBootstrapReceipt":{"workspaceId":"ws-bob","generation":2,"gatewayEpoch":3},
        "rustRuntimeStorage":{"format":"zuse-rust-engine-v1","storageIncarnationId":"original-storage"},
        "rustStorageEnrollment":{"phase":"committed","generation":2,"sourceSandboxId":"machine-bob","storageIncarnationId":"original-storage","credentialKeyThumbprint":"credential-key","signingKeyThumbprint":"signing-key"},
        "commandDeliveryMode":zuse_protocol::cloud_task_commands::MODE,"rustCommandAdmissionVersion":zuse_protocol::cloud_task_commands::VERSION,"rustCommandMailboxEnabled":true});
    sqlx::query("INSERT INTO zuse_rust_control_owners VALUES('workspace','ws-bob','rust',2,$1)")
        .bind(now)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE api_cloud_workspaces SET state='ready',desired_state='ready',runtime_state='online',request_config=$1,runtime_credential_hash=$2 WHERE workspace_id='ws-bob'").bind(config).bind(hash(token)).execute(&pool).await.unwrap();
    let signing = ed25519_dalek::SigningKey::from_bytes(&[74; 32]);
    let jwk = json!({"kty":"OKP","crv":"Ed25519","d":URL_SAFE_NO_PAD.encode([74;32]),"x":URL_SAFE_NO_PAD.encode(signing.verifying_key().as_bytes())});
    let objects = Arc::new(SlowStore {
        entered: Semaphore::new(0),
        release: Semaphore::new(0),
    });
    let control = Arc::new(
        RuntimeControl::from_pool(
            pool.clone(),
            MasterKey::parse(&URL_SAFE_NO_PAD.encode([75; 32])).unwrap(),
            TicketSigner::from_jwk(&jwk.to_string(), "https://api.example.test").unwrap(),
        )
        .with_fast_start_accounts(vec!["user-bob".into()])
        .with_transcript_objects(objects.clone()),
    );
    let upload:Upload=serde_json::from_value(json!({"sessionId":"session-bob","cursor":{"epoch":"original","version":1},"ciphertext":"fixture-ciphertext","ciphertextSha256":crate::transcript::digest(b"fixture-ciphertext")})).unwrap();
    let task_control = control.clone();
    let task_upload = upload.clone();
    let pending = tokio::spawn(async move {
        task_control
            .transcript_upload("ws-bob", token, task_upload, false, now)
            .await
    });
    timeout(Duration::from_secs(3), objects.entered.acquire())
        .await
        .unwrap()
        .unwrap()
        .forget();
    let gateway = timeout(
        Duration::from_millis(200),
        control.gateway_lease("ws-bob", "gateway-fixture", (2, 3)),
    )
    .await;
    objects.release.add_permits(1);
    let published = timeout(Duration::from_secs(3), pending)
        .await
        .unwrap()
        .unwrap();
    assert!(published.is_ok());
    assert!(
        gateway.is_ok(),
        "object PUT held the workspace lock and stalled runtime gateway readiness"
    );
    gateway.unwrap().unwrap();
    // A generation change may proceed while another PUT is outstanding, but
    // its old result must never advance the new runtime's transcript pointer.
    let task_control = control.clone();
    let mut newer = upload;
    newer.cursor.version = 2;
    let pending = tokio::spawn(async move {
        task_control
            .transcript_upload("ws-bob", token, newer, false, now)
            .await
    });
    timeout(Duration::from_secs(3), objects.entered.acquire())
        .await
        .unwrap()
        .unwrap()
        .forget();
    sqlx::query("UPDATE zuse_rust_control_owners SET generation=3 WHERE resource_id='ws-bob'")
        .execute(&pool)
        .await
        .unwrap();
    objects.release.add_permits(1);
    assert!(
        timeout(Duration::from_secs(3), pending)
            .await
            .unwrap()
            .unwrap()
            .is_err()
    );
    let version: i64 = sqlx::query_scalar(
        "SELECT stream_version FROM api_cloud_transcript_checkpoints WHERE workspace_id='ws-bob'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(version, 1);
    let intents: i64 = sqlx::query_scalar(
        "SELECT count(*)FROM zuse_rust_object_write_intents WHERE workspace_id='ws-bob'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        intents, 1,
        "late physical write must retain durable cleanup authority"
    );
    pool.close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .unwrap();
    admin.close().await;
}
