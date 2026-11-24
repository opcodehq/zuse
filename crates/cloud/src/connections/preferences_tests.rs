use super::{worker::*, *};
use zuse_protocol::cloud_computers::AgentId;

async fn fixture() -> (PgPool, PgPool, String) {
    let url = std::env::var("ZUSE_CLOUD_TEST_DATABASE_URL").expect("isolated PostgreSQL required");
    assert!(matches!(
        reqwest::Url::parse(&url).unwrap().host_str(),
        Some("localhost" | "127.0.0.1" | "::1")
    ));
    let admin = PgPool::connect(&url).await.unwrap();
    let schema = format!("connection_preferences_{}", uuid::Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&admin)
        .await
        .unwrap();
    let search = schema.clone();
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(6)
        .after_connect(move |connection, _| {
            let search = search.clone();
            Box::pin(async move {
                sqlx::query(&format!("SET search_path={search}"))
                    .execute(connection)
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
    (admin, pool, schema)
}

async fn cleanup(admin: PgPool, pool: PgPool, schema: String) {
    pool.close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .unwrap();
}

async fn migrate(pool: &PgPool) {
    sqlx::raw_sql(include_str!(
        "../../migrations/0055_agent_connection_preferences.sql"
    ))
    .execute(pool)
    .await
    .unwrap();
}

fn control(pool: &PgPool) -> Arc<ConnectionControl> {
    Arc::new(ConnectionControl::from_pool(
        pool.clone(),
        MasterKey::parse("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA").unwrap(),
        vec!["owner".into(), "organization:team".into()],
    ))
}

fn key_request(idempotency_key: &str, agent: AgentId) -> ConfigureConnection {
    ConfigureConnection {
        connection_id: None,
        agent,
        method: ConnectionMethod::ApiKey,
        secret: "fixture-never-public".into(),
        idempotency_key: idempotency_key.into(),
    }
}

#[tokio::test]
#[ignore = "requires explicit isolated localhost PostgreSQL"]
async fn connection_preferences_backfill_append_move_and_clear_within_owner() {
    let (admin, pool, schema) = fixture().await;
    sqlx::raw_sql(
        "INSERT INTO cloud_agent_connections(connection_id,owner_id,idempotency_key,request_digest,agent,method,state,provider_account_id,created_at,updated_at)
         VALUES ('last','owner','last','fixture','codex','device-code','connected','legacy-id',2,2),
                ('first','owner','first','fixture','codex','device-code','connected','legacy-id',2,2),
                ('gone','owner','gone','fixture','codex','device-code','revoked','legacy-id',1,1),
                ('team','organization:team','team','fixture','codex','api-key','connected',NULL,0,0)",
    )
    .execute(&pool)
    .await
    .unwrap();
    migrate(&pool).await;
    let control = control(&pool);
    let store = Store::from_pool(pool.clone()).with_shared_connections(control.clone());
    let listed = store.agent_connections("owner").await.unwrap();
    assert_eq!(
        listed
            .iter()
            .map(|row| (row.connection_id.as_str(), row.position, row.enabled))
            .collect::<Vec<_>>(),
        [("first", 0, true), ("last", 1, true)]
    );
    let now = chrono::Utc::now().timestamp_millis();
    let (one, two) = tokio::join!(
        control.configure(
            "owner",
            "owner",
            key_request("append-one", AgentId::Codex),
            now
        ),
        control.configure(
            "owner",
            "owner",
            key_request("append-two", AgentId::Claude),
            now
        )
    );
    let mut positions = [one.unwrap().position, two.unwrap().position];
    positions.sort_unstable();
    assert_eq!(positions, [2, 3]);
    let patch = UpdateConnection {
        position: Some(0),
        enabled: Some(false),
        alias: Some("  Work plan  ".into()),
    };
    for id in ["team", "gone"] {
        assert_eq!(
            control
                .update("owner", id, patch.clone(), now)
                .await
                .unwrap_err()
                .code,
            "agent_connection_not_found"
        );
    }
    let changed = control.update("owner", "last", patch, now).await.unwrap();
    assert_eq!(changed.position, 0);
    assert_eq!(changed.alias.as_deref(), Some("Work plan"));
    assert!(!changed.enabled);
    assert_eq!(changed.version, 1);
    let listed = store.agent_connections("owner").await.unwrap();
    assert_eq!(listed[0].connection_id, "last");
    assert_eq!(listed[1].connection_id, "first");
    assert_eq!(listed[1].position, 1);
    let team = store.agent_connections("organization:team").await.unwrap();
    assert_eq!((team[0].position, team[0].enabled), (0, true));

    let cleared = control
        .update(
            "owner",
            "last",
            UpdateConnection {
                position: Some(3),
                enabled: None,
                alias: Some(String::new()),
            },
            now,
        )
        .await
        .unwrap();
    assert_eq!(cleared.position, 3);
    assert_eq!(cleared.alias, None);
    assert!(!cleared.enabled);
    control.disconnect("owner", "first", now).await.unwrap();
    let listed = store.agent_connections("owner").await.unwrap();
    assert_eq!(
        listed.iter().map(|row| row.position).collect::<Vec<_>>(),
        [0, 1, 2]
    );
    assert_eq!(listed[2].connection_id, "last");
    cleanup(admin, pool, schema).await;
}

struct VerifiedAccount<'a> {
    id: Option<&'static str>,
    barrier: Option<&'a tokio::sync::Barrier>,
}

#[async_trait::async_trait]
impl AuthDriver for VerifiedAccount<'_> {
    async fn execute(
        &self,
        job: AuthJob,
        _: tokio::sync::mpsc::Sender<DeviceChallenge>,
    ) -> Result<VerifiedConnection, &'static str> {
        if let Some(barrier) = self.barrier {
            barrier.wait().await;
        }
        Ok(VerifiedConnection {
            credential: Zeroizing::new(
                json!({
                    "formatVersion":1,"agent":job.agent,"method":job.method,
                    "secret":job.secret.as_str(),"providerAccountId":self.id,
                })
                .to_string(),
            ),
            account_label: Some("same display label".into()),
            account_id: self.id.map(str::to_owned),
            expires_at: Some(chrono::Utc::now().timestamp_millis() + 3_600_000),
        })
    }
}

#[tokio::test]
#[ignore = "requires explicit isolated localhost PostgreSQL"]
async fn concurrent_verification_dedupes_when_and_only_when_owner_agent_and_account_match() {
    let (admin, pool, schema) = fixture().await;
    migrate(&pool).await;
    let control = control(&pool);
    let store = Store::from_pool(pool.clone()).with_shared_connections(control.clone());
    let now = chrono::Utc::now().timestamp_millis();
    let login = |key: &str| StartConnectionLogin {
        agent: AgentId::Codex,
        connection_id: None,
        idempotency_key: key.into(),
    };
    let first = control
        .start_login("owner", "owner", login("first"), now)
        .await
        .unwrap();
    let second = control
        .start_login("owner", "owner", login("second"), now)
        .await
        .unwrap();
    let pending = store
        .agent_connections_for_actor("owner", Some("owner"))
        .await
        .unwrap();
    assert_eq!(
        pending[0].pending_login_operation_id.as_deref(),
        Some(first.operation_id.as_str())
    );
    assert_eq!(
        pending[1].pending_login_operation_id.as_deref(),
        Some(second.operation_id.as_str())
    );
    let other_actor = store
        .agent_connections_for_actor("owner", Some("another-actor"))
        .await
        .unwrap();
    assert!(
        other_actor
            .iter()
            .all(|row| row.pending_login_operation_id.is_none())
    );
    let barrier = tokio::sync::Barrier::new(2);
    let mut driver = VerifiedAccount {
        id: Some("durable-provider-id"),
        barrier: Some(&barrier),
    };
    let (one, two) = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        tokio::join!(control.work_once(&driver), control.work_once(&driver))
    })
    .await
    .unwrap();
    assert!(one.unwrap());
    assert!(two.unwrap());
    let rows = store.agent_connections("owner").await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].state, ConnectionState::Connected);
    assert_eq!(rows[0].position, 0);
    assert!(rows[0].pending_login_operation_id.is_none());
    let operation = if rows[0].connection_id == first.connection_id {
        &second
    } else {
        &first
    };
    let outcome = control
        .operation("owner", &operation.operation_id, now)
        .await
        .unwrap();
    assert_eq!(outcome.state, LoginState::Error);
    assert_eq!(
        outcome.error_code.as_deref(),
        Some("agent_already_connected")
    );
    let erased: bool = sqlx::query_scalar(
        "SELECT state='revoked' AND credential_ciphertext IS NULL AND provider_account_id IS NULL FROM cloud_agent_connections WHERE connection_id=$1",
    ).bind(&operation.connection_id).fetch_one(&pool).await.unwrap();
    assert!(erased);

    driver.barrier = None;
    let same_account_key = control
        .configure(
            "owner",
            "owner",
            key_request("different-method", AgentId::Codex),
            now,
        )
        .await
        .unwrap();
    assert!(control.work_once(&driver).await.unwrap());
    let rows = store.agent_connections("owner").await.unwrap();
    let dupe = rows
        .iter()
        .find(|row| row.connection_id == same_account_key.connection_id)
        .unwrap();
    assert_eq!(dupe.error_code.as_deref(), Some("agent_already_connected"));
    assert!(
        !serde_json::to_string(&rows)
            .unwrap()
            .contains("durable-provider-id")
    );
    assert!(
        !serde_json::to_string(&rows)
            .unwrap()
            .contains("fixture-never-public")
    );

    control
        .start_login("organization:team", "owner", login("team"), now)
        .await
        .unwrap();
    assert!(control.work_once(&driver).await.unwrap());
    assert_eq!(
        store.agent_connections("organization:team").await.unwrap()[0].state,
        ConnectionState::Connected
    );
    control
        .configure(
            "owner",
            "owner",
            key_request("other-agent", AgentId::Claude),
            now,
        )
        .await
        .unwrap();
    assert!(control.work_once(&driver).await.unwrap());
    driver.id = None;
    for key in ["missing-account-one", "missing-account-two"] {
        control
            .configure("owner", "owner", key_request(key, AgentId::Codex), now)
            .await
            .unwrap();
        assert!(control.work_once(&driver).await.unwrap());
    }
    let rows = store.agent_connections("owner").await.unwrap();
    assert_eq!(
        rows.iter()
            .filter(|row| row.state == ConnectionState::Connected)
            .count(),
        4
    );
    cleanup(admin, pool, schema).await;
}
