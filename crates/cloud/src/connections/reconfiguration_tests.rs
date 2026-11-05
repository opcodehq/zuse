//! Real PostgreSQL replacement races; the verifier is controlled, never a live vendor.
use super::*;
use crate::connections::worker::{AuthDriver, AuthJob, DeviceChallenge, VerifiedConnection};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD as B64};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use tokio::sync::{Notify, mpsc};
use zuse_protocol::cloud_computers::AgentId;
struct Driver {
    calls: AtomicUsize,
    entered: Notify,
    release: Notify,
    gated: bool,
    reject: bool,
    account: &'static str,
}
impl Driver {
    fn new(gated: bool) -> Self {
        Self {
            calls: AtomicUsize::new(0),
            entered: Notify::new(),
            release: Notify::new(),
            gated,
            reject: false,
            account: "same-vendor-account",
        }
    }
}
#[async_trait::async_trait]
impl AuthDriver for Driver {
    async fn execute(
        &self,
        job: AuthJob,
        _: mpsc::Sender<DeviceChallenge>,
    ) -> Result<VerifiedConnection, &'static str> {
        assert!(!job.refresh);
        assert!(job.secret.starts_with("replacement-secret"));
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.entered.notify_one();
        if self.gated {
            self.release.notified().await;
        }
        if self.reject {
            return Err("auth_credentials_invalid");
        }
        Ok(VerifiedConnection{credential:Zeroizing::new(json!({"formatVersion":1,"agent":job.agent,"method":job.method,"secret":job.secret.as_str(),"providerAccountId":self.account}).to_string()),account_label:Some("Verified fixture account".into()),account_id:Some(self.account.into()),expires_at:Some(chrono::Utc::now().timestamp_millis()+3_600_000)})
    }
}
fn key() -> MasterKey {
    MasterKey::parse(&B64.encode([97; 32])).unwrap()
}
fn control(pool: PgPool) -> ConnectionControl {
    ConnectionControl::from_pool(pool, key(), vec!["owner".into(), "other-owner".into()])
}
async fn seed(pool: &PgPool, id: &str, agent: AgentId, method: ConnectionMethod) -> String {
    let raw=json!({"formatVersion":1,"agent":agent,"method":method,"secret":"old-credential-secret","providerAccountId":"same-vendor-account"}).to_string();
    let encrypted = key()
        .seal_api(&format!("agent-connection:owner:{id}"), &raw)
        .unwrap();
    let now = chrono::Utc::now().timestamp_millis();
    sqlx::query("INSERT INTO cloud_agent_connections(connection_id,owner_id,idempotency_key,request_digest,agent,method,state,credential_ciphertext,provider_account_id,expires_at,version,authority_epoch,created_at,updated_at) VALUES($1,'owner',$1,'original',$2,$3,'connected',$4,'same-vendor-account',$5,5,7,$6,$6)").bind(id).bind(agent.as_str()).bind(method.as_str()).bind(&encrypted).bind(now+3_600_000).bind(now).execute(pool).await.unwrap();
    encrypted
}
fn request(id: &str, agent: AgentId, method: ConnectionMethod, key: &str) -> ConfigureConnection {
    ConfigureConnection {
        connection_id: Some(id.into()),
        agent,
        method,
        secret: "replacement-secret-verified".into(),
        idempotency_key: key.into(),
    }
}
async fn row(pool: &PgPool, id: &str) -> Value {
    sqlx::query_scalar("SELECT to_jsonb(c) FROM cloud_agent_connections c WHERE connection_id=$1")
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
}
#[tokio::test]
#[ignore = "requires explicit isolated localhost PostgreSQL"]
async fn existing_non_device_credentials_verify_before_promotion_and_fence_refresh_disconnect() {
    let url = std::env::var("ZUSE_CLOUD_TEST_DATABASE_URL").expect("isolated PostgreSQL required");
    assert!(matches!(
        reqwest::Url::parse(&url).unwrap().host_str(),
        Some("localhost" | "127.0.0.1" | "::1")
    ));
    let admin = PgPool::connect(&url).await.unwrap();
    let schema = format!("reconfigure_{}", uuid::Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&admin)
        .await
        .unwrap();
    let search = schema.clone();
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(6)
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
    sqlx::raw_sql(include_str!("../../migrations/0009_computers.sql"))
        .execute(&pool)
        .await
        .unwrap();
    sqlx::raw_sql(include_str!(
        "../../migrations/0024_credential_delivery_journal.sql"
    ))
    .execute(&pool)
    .await
    .unwrap();
    let authority = Arc::new(control(pool.clone()));
    for (id, agent, method) in [
        (
            "claude-selected",
            AgentId::Claude,
            ConnectionMethod::SetupToken,
        ),
        ("codex-selected", AgentId::Codex, ConnectionMethod::ApiKey),
        ("grok-selected", AgentId::Grok, ConnectionMethod::ApiKey),
    ] {
        let old = seed(&pool, id, agent, method).await;
        let now = chrono::Utc::now().timestamp_millis();
        let admitted = authority
            .configure("owner", "owner", request(id, agent, method, id), now)
            .await
            .unwrap();
        assert_eq!(admitted.connection_id, id);
        assert_eq!(admitted.state, ConnectionState::Pending);
        assert_eq!(admitted.credential_update_key.as_deref(), Some(id));
        let listed = Store::from_pool(pool.clone())
            .with_shared_connections(authority.clone())
            .agent_connections("owner")
            .await
            .unwrap();
        let exact = listed.iter().find(|row| row.connection_id == id).unwrap();
        assert_eq!(exact.credential_update_key.as_deref(), Some(id));
        assert_eq!(
            exact.credential_update.as_ref().unwrap().operation_id,
            admitted.credential_update.as_ref().unwrap().operation_id
        );
        assert_eq!(exact.state, ConnectionState::Pending);
        assert!(
            !serde_json::to_string(&listed)
                .unwrap()
                .contains("replacement-secret")
        );
        let operation = admitted.credential_update.unwrap();
        assert_eq!(operation.state, LoginState::Queued);
        let before = row(&pool, id).await;
        assert_eq!(before["credential_ciphertext"], old);
        assert_eq!(before["version"], 5);
        assert_eq!(before["authority_epoch"], 7);
        assert_eq!(before["state"], "connected");
        let queued: i64 =
            sqlx::query_scalar("SELECT count(*) FROM cloud_auth_operations WHERE state='queued'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(
            queued, 0,
            "A queued-only old worker must not verify the old credential instead"
        );
        let encrypted: String = sqlx::query_scalar(
            "SELECT operation_ciphertext FROM cloud_auth_operations WHERE operation_id=$1",
        )
        .bind(&operation.operation_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(!encrypted.contains("replacement-secret"));
        let bytes = key()
            .open_api(
                &format!("agent-login:owner:{}", operation.operation_id),
                &encrypted,
            )
            .unwrap();
        assert!(
            std::str::from_utf8(&bytes)
                .unwrap()
                .contains("replacement-secret")
        );
        assert!(
            !serde_json::to_string(
                &authority
                    .operation("owner", &operation.operation_id, now)
                    .await
                    .unwrap()
            )
            .unwrap()
            .contains("replacement-secret")
        );
        let wrong_agent = if agent == AgentId::Codex {
            AgentId::Grok
        } else {
            AgentId::Codex
        };
        assert_eq!(
            authority
                .configure(
                    "owner",
                    "owner",
                    request(id, wrong_agent, ConnectionMethod::ApiKey, "wrong-agent"),
                    now
                )
                .await
                .unwrap_err()
                .code,
            "agent_connection_not_found"
        );
        if method == ConnectionMethod::SetupToken {
            assert_eq!(
                authority
                    .configure(
                        "owner",
                        "owner",
                        request(id, agent, ConnectionMethod::ApiKey, "wrong-method"),
                        now
                    )
                    .await
                    .unwrap_err()
                    .code,
                "agent_connection_not_found"
            );
        }
        let driver = Driver::new(false);
        assert!(authority.work_once(&driver).await.unwrap());
        assert_eq!(driver.calls.load(Ordering::SeqCst), 1);
        let after = row(&pool, id).await;
        assert_eq!(after["version"], 6);
        assert_eq!(after["authority_epoch"], 8);
        assert_eq!(after["state"], "connected");
        assert_ne!(after["credential_ciphertext"], old);
        assert_eq!(after["agent"], agent.as_str());
        assert_eq!(after["method"], method.as_str());
        let retry = authority
            .configure("owner", "owner", request(id, agent, method, id), now)
            .await
            .unwrap();
        assert_eq!(
            retry.credential_update.unwrap().operation_id,
            operation.operation_id
        );
        assert!(!authority.work_once(&driver).await.unwrap());
        assert_eq!(row(&pool, id).await["version"], 6);
        let mut changed = request(id, agent, method, id);
        changed.secret = "replacement-secret-different".into();
        assert_eq!(
            authority
                .configure("owner", "owner", changed, now)
                .await
                .unwrap_err()
                .code,
            "agent_connection_idempotency_conflict"
        );
        assert_eq!(
            authority
                .configure(
                    "other-owner",
                    "other-owner",
                    request(id, agent, method, "cross-owner"),
                    now
                )
                .await
                .unwrap_err()
                .code,
            "agent_connection_not_found"
        );
        let pending_cipher: Option<String> = sqlx::query_scalar(
            "SELECT operation_ciphertext FROM cloud_auth_operations WHERE operation_id=$1",
        )
        .bind(operation.operation_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(pending_cipher.is_none());
    }
    // Invalid verification and known-account mismatch preserve every old connection field.
    for (id, reject, account) in [
        ("invalid-secret", true, "same-vendor-account"),
        ("wrong-account", false, "different-account"),
    ] {
        seed(&pool, id, AgentId::Claude, ConnectionMethod::SetupToken).await;
        let before = row(&pool, id).await;
        let admitted = authority
            .configure(
                "owner",
                "owner",
                request(id, AgentId::Claude, ConnectionMethod::SetupToken, id),
                chrono::Utc::now().timestamp_millis(),
            )
            .await
            .unwrap();
        let driver = Driver {
            reject,
            account,
            ..Driver::new(false)
        };
        authority.work_once(&driver).await.unwrap();
        assert_eq!(row(&pool, id).await, before);
        assert_eq!(
            authority
                .operation(
                    "owner",
                    &admitted.credential_update.unwrap().operation_id,
                    chrono::Utc::now().timestamp_millis()
                )
                .await
                .unwrap()
                .state,
            LoginState::Error
        );
    }
    // The production non-device refresh/invalidation path races the verified proposal.
    for (id, before_dispatch, disconnect) in [
        ("refresh-before", true, false),
        ("refresh-during", false, false),
        ("disconnect-during", false, true),
        ("cancel-during", false, false),
    ] {
        let old = seed(&pool, id, AgentId::Claude, ConnectionMethod::SetupToken).await;
        let admitted = authority
            .configure(
                "owner",
                "owner",
                request(id, AgentId::Claude, ConnectionMethod::SetupToken, id),
                chrono::Utc::now().timestamp_millis(),
            )
            .await
            .unwrap();
        let operation = admitted.credential_update.unwrap();
        let driver = Arc::new(Driver::new(!before_dispatch));
        if before_dispatch {
            assert!(
                authority
                    .credential("owner", id, "claude", true, driver.as_ref())
                    .await
                    .is_err()
            );
            authority.work_once(driver.as_ref()).await.unwrap();
            assert_eq!(driver.calls.load(Ordering::SeqCst), 0);
        } else {
            let worker = {
                let authority = authority.clone();
                let driver = driver.clone();
                tokio::spawn(async move { authority.work_once(driver.as_ref()).await.unwrap() })
            };
            tokio::time::timeout(std::time::Duration::from_secs(5), driver.entered.notified())
                .await
                .unwrap();
            if disconnect {
                authority
                    .disconnect("owner", id, chrono::Utc::now().timestamp_millis())
                    .await
                    .unwrap();
            } else if id == "cancel-during" {
                authority
                    .cancel(
                        "owner",
                        &operation.operation_id,
                        chrono::Utc::now().timestamp_millis(),
                    )
                    .await
                    .unwrap();
            } else {
                assert!(
                    authority
                        .credential("owner", id, "claude", true, driver.as_ref())
                        .await
                        .is_err()
                );
            }
            driver.release.notify_one();
            assert!(worker.await.unwrap());
        }
        let current = row(&pool, id).await;
        if disconnect {
            assert_eq!(current["state"], "revoked");
            assert!(current["credential_ciphertext"].is_null());
            assert_eq!(
                authority
                    .configure(
                        "owner",
                        "owner",
                        request(
                            id,
                            AgentId::Claude,
                            ConnectionMethod::SetupToken,
                            "cannot-revive"
                        ),
                        chrono::Utc::now().timestamp_millis()
                    )
                    .await
                    .unwrap_err()
                    .code,
                "agent_connection_not_found"
            );
        } else {
            assert_eq!(current["credential_ciphertext"], old);
            assert_ne!(current["version"], 7);
        }
    }
    pool.close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .unwrap();
    admin.close().await;
}
