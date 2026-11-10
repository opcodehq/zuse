//! Real process death and PostgreSQL checkpoint recovery. Credentials are fixtures, not vendor acceptance.
use super::*;
use crate::connections::worker::{AuthDriver, AuthJob, DeviceChallenge, VerifiedConnection};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD as B64};
use serde_json::json;
use sqlx::PgPool;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::mpsc;
struct NeverRefresh(AtomicUsize);
#[async_trait::async_trait]
impl AuthDriver for NeverRefresh {
    async fn execute(
        &self,
        _: AuthJob,
        _: mpsc::Sender<DeviceChallenge>,
    ) -> Result<VerifiedConnection, &'static str> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Err("must_not_repeat_vendor_refresh")
    }
}
async fn pool(url: &str, schema: &str) -> PgPool {
    assert!(
        schema.starts_with("credential_journal_")
            && schema
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_')
    );
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
fn master() -> crate::crypto::MasterKey {
    crate::crypto::MasterKey::parse(&B64.encode([77; 32])).unwrap()
}
fn control(pool: PgPool) -> ConnectionControl {
    ConnectionControl::from_pool(pool, master(), vec!["member".into()])
}
fn canonical(token: &str) -> String {
    json!({"formatVersion":1,"agent":"codex","method":"device-code","accessToken":token,"refreshToken":"fixture-refresh","providerAccountId":"same-account","nativeAuth":{}}).to_string()
}
#[tokio::test]
#[ignore = "helper subprocess: requires parent-provided isolated PostgreSQL schema"]
async fn rotation_crash_child() {
    let Some(schema) = std::env::var("ZUSE_CREDENTIAL_TEST_SCHEMA").ok() else {
        return;
    };
    let url = std::env::var("ZUSE_CLOUD_TEST_DATABASE_URL").unwrap();
    assert!(matches!(
        reqwest::Url::parse(&url).unwrap().host_str(),
        Some("localhost" | "127.0.0.1" | "::1")
    ));
    let authority = control(pool(&url, &schema).await);
    let now = chrono::Utc::now().timestamp_millis();
    let rotation = authority
        .start_rotation("member", "connection", "codex", 1, 3, now)
        .await
        .unwrap()
        .unwrap();
    if std::env::var("ZUSE_CREDENTIAL_TEST_CRASH_STAGE").as_deref() == Ok("result") {
        authority
            .checkpoint_rotation(
                &rotation,
                &VerifiedConnection {
                    credential: Zeroizing::new(canonical("fixture-rotated-access")),
                    account_id: Some("same-account".into()),
                    account_label: None,
                    expires_at: Some(now + 3_600_000),
                },
                now,
            )
            .await
            .unwrap();
    }
    // This kills the real authority subprocess, bypassing Drop, after the durable boundary.
    unsafe {
        libc::kill(libc::getpid(), libc::SIGKILL);
    }
    unreachable!()
}
#[tokio::test]
#[ignore = "requires explicit isolated localhost PostgreSQL"]
async fn refresh_result_survives_process_death_and_uncertain_dispatch_never_replays() {
    let url = std::env::var("ZUSE_CLOUD_TEST_DATABASE_URL").expect("isolated PostgreSQL required");
    assert!(matches!(
        reqwest::Url::parse(&url).unwrap().host_str(),
        Some("localhost" | "127.0.0.1" | "::1")
    ));
    let admin = PgPool::connect(&url).await.unwrap();
    for stage in ["result", "dispatching"] {
        let schema = format!("credential_journal_{}", uuid::Uuid::new_v4().simple());
        sqlx::query(&format!("CREATE SCHEMA {schema}"))
            .execute(&admin)
            .await
            .unwrap();
        let pool = pool(&url, &schema).await;
        sqlx::raw_sql("CREATE TABLE cloud_agent_connections(connection_id text PRIMARY KEY,owner_id text,agent text,method text,state text,credential_ciphertext text,version bigint,authority_epoch bigint,expires_at bigint,refresh_lease_id text,refresh_lease_expires_at bigint,provider_account_id text,account_label text,last_error_code text,updated_at bigint)").execute(&pool).await.unwrap();
        sqlx::raw_sql(include_str!(
            "../../migrations/0024_credential_delivery_journal.sql"
        ))
        .execute(&pool)
        .await
        .unwrap();
        let ciphertext = master()
            .seal_api(
                "agent-connection:member:connection",
                &canonical("fixture-old-access"),
            )
            .unwrap();
        let now = chrono::Utc::now().timestamp_millis();
        sqlx::query("INSERT INTO cloud_agent_connections(connection_id,owner_id,agent,method,state,credential_ciphertext,version,authority_epoch,expires_at,provider_account_id,updated_at) VALUES('connection','member','codex','device-code','connected',$1,1,3,$2,'same-account',$3)").bind(ciphertext).bind(now+30_000).bind(now).execute(&pool).await.unwrap();
        let binary = std::env::current_exe().unwrap();
        let child = std::process::Command::new(binary)
            .args([
                "--exact",
                "connections::refresh_journal::tests::rotation_crash_child",
                "--ignored",
                "--nocapture",
            ])
            .env("ZUSE_CLOUD_TEST_DATABASE_URL", &url)
            .env("ZUSE_CREDENTIAL_TEST_SCHEMA", &schema)
            .env("ZUSE_CREDENTIAL_TEST_CRASH_STAGE", stage)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap();
        use std::os::unix::process::ExitStatusExt;
        assert_eq!(
            child.signal(),
            Some(libc::SIGKILL),
            "must prove actual authority process death"
        );
        let authority = control(pool.clone());
        let driver = NeverRefresh(AtomicUsize::new(0));
        if stage == "dispatching" {
            // Simulate elapsed lease time in an isolated DB fixture, not vendor expiry.
            sqlx::query("UPDATE zuse_rust_credential_rotations SET lease_expires_at=0")
                .execute(&pool)
                .await
                .unwrap();
            assert_eq!(
                authority
                    .credential("member", "connection", "codex", true, &driver)
                    .await
                    .err()
                    .unwrap()
                    .code,
                "agent_connection_refresh_uncertain"
            );
            assert!(
                authority
                    .credential("member", "connection", "codex", true, &driver)
                    .await
                    .is_err()
            );
        } else {
            let value = authority
                .credential("member", "connection", "codex", false, &driver)
                .await
                .unwrap();
            assert_eq!(value.secret.as_str(), "fixture-rotated-access");
            let retained: Option<String> =
                sqlx::query_scalar("SELECT result_ciphertext FROM zuse_rust_credential_rotations")
                    .fetch_one(&pool)
                    .await
                    .unwrap();
            assert!(
                retained.is_none(),
                "atomic applied checkpoint erases duplicate renewable body"
            );
        }
        assert_eq!(
            driver.0.load(Ordering::SeqCst),
            0,
            "neither recoverable result nor uncertainty may call vendor again"
        );
        let state: (String, i64, i64) =
            sqlx::query_as("SELECT state,version,authority_epoch FROM cloud_agent_connections")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(
            state,
            if stage == "result" {
                ("connected".into(), 2, 3)
            } else {
                ("reconnect-required".into(), 2, 4)
            }
        );
        pool.close().await;
        sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
            .execute(&admin)
            .await
            .unwrap();
    }
    admin.close().await;
}
