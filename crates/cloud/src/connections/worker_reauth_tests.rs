//! Existing-connection reauthentication keeps task identity; no vendor call in this fixture.
use super::*;
use crate::crypto::MasterKey;
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD as B64};
use zuse_protocol::cloud_computers::AgentId;
struct AccountDriver(&'static str);
#[async_trait::async_trait]
impl AuthDriver for AccountDriver {
    async fn execute(
        &self,
        job: AuthJob,
        _: mpsc::Sender<DeviceChallenge>,
    ) -> Result<VerifiedConnection, &'static str> {
        assert!(!job.refresh);
        assert_eq!(job.agent, "codex");
        Ok(VerifiedConnection{credential:Zeroizing::new(json!({"formatVersion":1,"agent":"codex","method":"device-code","accessToken":"fixture-new-access","refreshToken":"fixture-new-refresh","providerAccountId":self.0,"nativeAuth":{}}).to_string()),account_id:Some(self.0.into()),account_label:Some("Fixture account".into()),expires_at:Some(chrono::Utc::now().timestamp_millis()+3_600_000)})
    }
}
fn key() -> MasterKey {
    MasterKey::parse(&B64.encode([78; 32])).unwrap()
}
fn authority(pool: sqlx::PgPool) -> ConnectionControl {
    ConnectionControl::from_pool(pool, key(), vec!["owner".into(), "another-owner".into()])
}
#[tokio::test]
#[ignore = "requires explicit isolated localhost PostgreSQL"]
async fn reauthentication_preserves_selected_identity_rejects_account_swap_and_allows_explicit_add()
{
    let url = std::env::var("ZUSE_CLOUD_TEST_DATABASE_URL").expect("isolated PostgreSQL required");
    assert!(matches!(
        reqwest::Url::parse(&url).unwrap().host_str(),
        Some("localhost" | "127.0.0.1" | "::1")
    ));
    let admin = sqlx::PgPool::connect(&url).await.unwrap();
    let schema = format!("reauth_{}", uuid::Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&admin)
        .await
        .unwrap();
    let search = schema.clone();
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(4)
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
    let now = chrono::Utc::now().timestamp_millis();
    let canonical=json!({"formatVersion":1,"agent":"codex","method":"device-code","accessToken":"fixture-original-access","refreshToken":"fixture-original-refresh","providerAccountId":"original-account","nativeAuth":{}}).to_string();
    let old = key()
        .seal_api("agent-connection:owner:task-pinned-connection", &canonical)
        .unwrap();
    sqlx::query("INSERT INTO cloud_agent_connections(connection_id,owner_id,idempotency_key,request_digest,agent,method,state,provider_account_id,credential_ciphertext,version,authority_epoch,created_at,updated_at) VALUES('task-pinned-connection','owner','original','device-code','codex','device-code','reconnect-required','original-account',$1,5,7,$2,$2)").bind(&old).bind(now).execute(&pool).await.unwrap();
    let control = authority(pool.clone());
    let login = control
        .start_login(
            "owner",
            "owner",
            StartConnectionLogin {
                agent: AgentId::Codex,
                connection_id: Some("task-pinned-connection".into()),
                idempotency_key: "wrong-account".into(),
            },
            now,
        )
        .await
        .unwrap();
    assert_eq!(login.connection_id, "task-pinned-connection");
    control
        .work_once(&AccountDriver("different-account"))
        .await
        .unwrap();
    let failed = control
        .operation(
            "owner",
            &login.operation_id,
            chrono::Utc::now().timestamp_millis(),
        )
        .await
        .unwrap();
    assert_eq!(failed.state, LoginState::Error);
    assert_eq!(failed.error_code.as_deref(), Some("auth_account_mismatch"));
    let retained:(String,i64,i64,Option<String>,String)=sqlx::query_as("SELECT state,version,authority_epoch,provider_account_id,credential_ciphertext FROM cloud_agent_connections WHERE connection_id='task-pinned-connection'").fetch_one(&pool).await.unwrap();
    assert_eq!(
        retained,
        (
            "reconnect-required".into(),
            5,
            7,
            Some("original-account".into()),
            old
        )
    );
    let correct = control
        .start_login(
            "owner",
            "owner",
            StartConnectionLogin {
                agent: AgentId::Codex,
                connection_id: Some("task-pinned-connection".into()),
                idempotency_key: "same-account".into(),
            },
            now,
        )
        .await
        .unwrap();
    control
        .work_once(&AccountDriver("original-account"))
        .await
        .unwrap();
    assert_eq!(
        control
            .operation(
                "owner",
                &correct.operation_id,
                chrono::Utc::now().timestamp_millis()
            )
            .await
            .unwrap()
            .state,
        LoginState::Connected
    );
    // Fresh authority instance has no prior in-memory account cache. It still reads
    // the original selected vendor identity and a newly fenced authority epoch.
    let cold = authority(pool.clone());
    let credential = cold
        .credential(
            "owner",
            "task-pinned-connection",
            "codex",
            false,
            &AccountDriver("must-not-execute"),
        )
        .await
        .unwrap();
    assert_eq!(credential.account_id.as_deref(), Some("original-account"));
    assert_eq!(credential.authority_epoch, 8);
    let added = control
        .start_login(
            "owner",
            "owner",
            StartConnectionLogin {
                agent: AgentId::Codex,
                connection_id: None,
                idempotency_key: "explicit-add".into(),
            },
            now,
        )
        .await
        .unwrap();
    assert_ne!(added.connection_id, "task-pinned-connection");
    control
        .work_once(&AccountDriver("different-account"))
        .await
        .unwrap();
    let ids:Vec<(String,String,i64)>=sqlx::query_as("SELECT connection_id,provider_account_id,authority_epoch FROM cloud_agent_connections ORDER BY connection_id").fetch_all(&pool).await.unwrap();
    assert_eq!(ids.len(), 2);
    assert!(
        ids.iter()
            .any(|(id, account, epoch)| id == "task-pinned-connection"
                && account == "original-account"
                && *epoch == 8)
    );
    assert!(
        ids.iter()
            .any(|(id, account, _)| id == &added.connection_id && account == "different-account")
    );
    assert_eq!(
        control
            .start_login(
                "another-owner",
                "another-owner",
                StartConnectionLogin {
                    agent: AgentId::Codex,
                    connection_id: Some("task-pinned-connection".into()),
                    idempotency_key: "cross-owner".into()
                },
                now
            )
            .await
            .err()
            .unwrap()
            .code,
        "agent_connection_not_found"
    );
    assert_eq!(
        control
            .start_login(
                "owner",
                "owner",
                StartConnectionLogin {
                    agent: AgentId::Grok,
                    connection_id: Some("task-pinned-connection".into()),
                    idempotency_key: "cross-agent".into()
                },
                now
            )
            .await
            .err()
            .unwrap()
            .code,
        "agent_connection_not_found"
    );
    sqlx::query("UPDATE cloud_agent_connections SET provider_account_id=NULL WHERE connection_id='task-pinned-connection'").execute(&pool).await.unwrap();
    let unknown = control
        .start_login(
            "owner",
            "owner",
            StartConnectionLogin {
                agent: AgentId::Codex,
                connection_id: Some("task-pinned-connection".into()),
                idempotency_key: "unknown-legacy-identity".into(),
            },
            now,
        )
        .await
        .unwrap();
    control
        .work_once(&AccountDriver("different-account"))
        .await
        .unwrap();
    let unknown = control
        .operation(
            "owner",
            &unknown.operation_id,
            chrono::Utc::now().timestamp_millis(),
        )
        .await
        .unwrap();
    assert_eq!(
        unknown.error_code.as_deref(),
        Some("auth_identity_review_required")
    );
    let retained:(Option<String>,i64)=sqlx::query_as("SELECT provider_account_id,authority_epoch FROM cloud_agent_connections WHERE connection_id='task-pinned-connection'").fetch_one(&pool).await.unwrap();
    assert_eq!(retained, (None, 8));
    // A legacy import can have default version/epoch yet already be connected.
    // NULL identity is not permission to silently attach a new provider account.
    let imported = key()
        .seal_api("agent-connection:owner:unknown-import", &canonical)
        .unwrap();
    sqlx::query("INSERT INTO cloud_agent_connections(connection_id,owner_id,idempotency_key,request_digest,agent,method,state,credential_ciphertext,version,authority_epoch,created_at,updated_at) VALUES('unknown-import','owner','legacy-import','device-code','codex','device-code','connected',$1,1,1,$2,$2)").bind(&imported).bind(now).execute(&pool).await.unwrap();
    let imported_login = control
        .start_login(
            "owner",
            "owner",
            StartConnectionLogin {
                agent: AgentId::Codex,
                connection_id: Some("unknown-import".into()),
                idempotency_key: "unknown-default-version".into(),
            },
            now,
        )
        .await
        .unwrap();
    control
        .work_once(&AccountDriver("different-account"))
        .await
        .unwrap();
    let imported_result = control
        .operation(
            "owner",
            &imported_login.operation_id,
            chrono::Utc::now().timestamp_millis(),
        )
        .await
        .unwrap();
    assert_eq!(
        imported_result.error_code.as_deref(),
        Some("auth_identity_review_required")
    );
    let unchanged:(Option<String>,i64,i64,String)=sqlx::query_as("SELECT provider_account_id,version,authority_epoch,credential_ciphertext FROM cloud_agent_connections WHERE connection_id='unknown-import'").fetch_one(&pool).await.unwrap();
    assert_eq!(unchanged, (None, 1, 1, imported));
    pool.close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .unwrap();
    admin.close().await;
}
