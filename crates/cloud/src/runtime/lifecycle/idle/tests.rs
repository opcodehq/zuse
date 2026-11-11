use super::*;
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};

fn config() -> Value {
    json!({
        "runtimeGeneration":2,"gatewayEpoch":2,"runtimeRpcProtocol":"zuse-rpc-v1",
        "commandDeliveryMode":zuse_protocol::cloud_task_commands::MODE,
        "rustCommandAdmissionVersion":2,"rustCommandMailboxEnabled":true,
        "runtimeBootstrapReceipt":{"workspaceId":"ws-bob","generation":2,"gatewayEpoch":2},
        "runtimeCredentialKeyThumbprint":"credential-key","runtimeSigningKeyThumbprint":"signing-key",
        "rustRuntimeStorage":{"storageIncarnationId":"retained-disk"},
        "rustStorageEnrollment":{"phase":"committed","generation":2,"sourceSandboxId":"machine-bob",
            "storageIncarnationId":"retained-disk","credentialKeyThumbprint":"credential-key","signingKeyThumbprint":"signing-key",
            "capabilities":[CAPABILITY,"rust-session-commands-v1",zuse_protocol::cloud_task_commands::CAPABILITY]}
    })
}
fn control(pool: PgPool) -> RuntimeControl {
    let signing = ed25519_dalek::SigningKey::from_bytes(&[71; 32]);
    let jwk = json!({"kty":"OKP","crv":"Ed25519","d":URL_SAFE_NO_PAD.encode([71;32]),"x":URL_SAFE_NO_PAD.encode(signing.verifying_key().as_bytes())});
    let control = RuntimeControl::from_pool(
        pool,
        MasterKey::parse(&URL_SAFE_NO_PAD.encode([72; 32])).unwrap(),
        TicketSigner::from_jwk(&jwk.to_string(), "https://api.example.test").unwrap(),
    )
    .with_creation_accounts(vec!["user-bob".into()]);
    // The scheduler tests use real lifecycle/job DDL and the execution columns
    // they read; schema/provisioning readiness has independent integration tests.
    control.task_command_schema_ready.set(true).unwrap();
    control
}
#[tokio::test]
#[ignore = "requires explicit isolated localhost PostgreSQL"]
async fn idle_deadline_respects_recent_work_busy_states_scope_and_durable_pause() {
    let url = std::env::var("ZUSE_CLOUD_TEST_DATABASE_URL").unwrap();
    assert!(matches!(
        reqwest::Url::parse(&url).unwrap().host_str(),
        Some("127.0.0.1" | "localhost" | "::1")
    ));
    let admin = PgPool::connect(&url).await.unwrap();
    let schema = format!("idle_{}", uuid::Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&admin)
        .await
        .unwrap();
    let scope = schema.clone();
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(5)
        .after_connect(move |connection, _| {
            let scope = scope.clone();
            Box::pin(async move {
                sqlx::query(&format!("SET search_path={scope}"))
                    .execute(connection)
                    .await?;
                Ok(())
            })
        })
        .connect(&url)
        .await
        .unwrap();
    for ddl in [
        include_str!("../../../../tests/fixtures/legacy-catalog.sql"),
        jobs::MIGRATION,
        MIGRATION,
        include_str!("../../../../migrations/0012_provider_receipts.sql"),
    ] {
        sqlx::raw_sql(ddl).execute(&pool).await.unwrap();
    }
    sqlx::raw_sql("CREATE TABLE zuse_rust_task_command_execution(workspace_id text NOT NULL,command_id text NOT NULL,state text NOT NULL,updated_at bigint NOT NULL,PRIMARY KEY(workspace_id,command_id)); INSERT INTO zuse_rust_control_owners VALUES('workspace','ws-bob','rust',2,0)").execute(&pool).await.unwrap();
    let control = std::sync::Arc::new(control(pool.clone()));
    let now = 2_000_000;
    let grace = crate::runtime::activity::COALESCE_MS;
    sqlx::query("UPDATE api_cloud_workspaces SET request_config=$1,last_activity_at=$2,next_action_at=$3 WHERE workspace_id='ws-bob'")
        .bind(config()).bind(now-600_000).bind(now-1).execute(&pool).await.unwrap();
    assert!(
        !control.idle_sleep_once(now).await.unwrap(),
        "coalesced input gets the full ten minutes"
    );
    sqlx::query("UPDATE api_cloud_workspaces SET last_activity_at=1,next_action_at=0 WHERE workspace_id='ws-bob'").execute(&pool).await.unwrap();
    for state in [
        "queued",
        "starting",
        "running",
        "waiting-input",
        "outcome-unknown",
    ] {
        sqlx::query("INSERT INTO zuse_rust_task_command_execution VALUES('ws-bob','held',$1,1) ON CONFLICT(workspace_id,command_id) DO UPDATE SET state=excluded.state")
            .bind(state).execute(&pool).await.unwrap();
        assert!(
            !control.idle_sleep_once(now).await.unwrap(),
            "must not sleep {state}"
        );
    }
    sqlx::query("UPDATE zuse_rust_task_command_execution SET state='completed',updated_at=$1")
        .bind(now - 300_000)
        .execute(&pool)
        .await
        .unwrap();
    assert!(
        !control.idle_sleep_once(now).await.unwrap(),
        "upgraded API must notice recent completed work even with old workspace timestamp"
    );
    sqlx::query("UPDATE zuse_rust_task_command_execution SET updated_at=1")
        .execute(&pool)
        .await
        .unwrap();
    for patch in [
        json!({"runtimeReleaseChangeRequested":true}),
        json!({"runtimeActivation":{"phase":"preparing"}}),
        json!({"rustRecreation":{"operationId":"recreate","phase":"capturing"}}),
        json!({"rustStorageEnrollment":null}),
    ] {
        let mut row = config();
        for (key, value) in patch.as_object().unwrap() {
            row[key] = value.clone();
        }
        sqlx::query(
            "UPDATE api_cloud_workspaces SET request_config=$1 WHERE workspace_id='ws-bob'",
        )
        .bind(row)
        .execute(&pool)
        .await
        .unwrap();
        assert!(
            !control.idle_sleep_once(now).await.unwrap(),
            "unsupported/repairing computers must not block or be paused"
        );
    }
    sqlx::query("UPDATE api_cloud_workspaces SET request_config=$1,state='setup' WHERE workspace_id='ws-bob'").bind(config()).execute(&pool).await.unwrap();
    assert!(
        !control.idle_sleep_once(now).await.unwrap(),
        "failed preparation is a separate recovery path"
    );
    sqlx::query("UPDATE api_cloud_workspaces SET state='ready' WHERE workspace_id='ws-bob'")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE zuse_rust_control_owners SET generation=3 WHERE resource_id='ws-bob'")
        .execute(&pool)
        .await
        .unwrap();
    assert!(
        !control.idle_sleep_once(now).await.unwrap(),
        "generation fence"
    );
    sqlx::query("UPDATE zuse_rust_control_owners SET generation=2 WHERE resource_id='ws-bob'")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO api_cloud_workspace_launch_intents VALUES('ws-bob','user-bob','chat-bob','session-bob','turn','launch','synthetic',999999999,0)").execute(&pool).await.unwrap();
    assert!(
        !control.idle_sleep_once(now).await.unwrap(),
        "launch must finish first"
    );
    sqlx::query("DELETE FROM api_cloud_workspace_launch_intents")
        .execute(&pool)
        .await
        .unwrap();
    // A meaningful request wins the workspace lock before idle admission.
    let mut held = pool.begin().await.unwrap();
    locking::workspace(&mut held, "ws-bob", true).await.unwrap();
    let pending = {
        let control = control.clone();
        tokio::spawn(async move { control.idle_sleep_once(now).await })
    };
    let mut blocked = false;
    for _ in 0..200 {
        blocked=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE pid<>pg_backend_pid() AND wait_event_type='Lock' AND query LIKE 'WITH authority AS MATERIALIZED%')").fetch_one(&admin).await.unwrap();
        if blocked {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert!(
        blocked,
        "observe actual database lock race rather than assuming scheduling order"
    );
    sqlx::query("UPDATE api_cloud_workspaces SET last_activity_at=$1 WHERE workspace_id='ws-bob'")
        .bind(now)
        .execute(&mut *held)
        .await
        .unwrap();
    held.commit().await.unwrap();
    assert!(
        !pending.await.unwrap().unwrap(),
        "locked recheck must honor concurrent activity"
    );
    sqlx::query("UPDATE api_cloud_workspaces SET last_activity_at=1 WHERE workspace_id='ws-bob'")
        .execute(&pool)
        .await
        .unwrap();
    let (a, b) = tokio::join!(control.idle_sleep_once(now), control.idle_sleep_once(now));
    assert_eq!(
        usize::from(a.unwrap()) + usize::from(b.unwrap()),
        1,
        "two workers must admit one pause"
    );
    let row: Value = sqlx::query_scalar(
        "SELECT to_jsonb(w) FROM api_cloud_workspaces w WHERE workspace_id='ws-bob'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(row["state"], "pausing");
    assert_eq!(row["desired_state"], "paused");
    assert_eq!(
        row["request_config"]["rustLifecycleOperation"]["reason"],
        "idle-timeout"
    );
    assert!(!control.idle_sleep_once(now).await.unwrap());
    let counts:(i64,i64)=sqlx::query_as("SELECT (SELECT count(*)FROM zuse_rust_lifecycle_operations),(SELECT count(*)FROM zuse_rust_jobs WHERE kind='workspace-lifecycle')").fetch_one(&pool).await.unwrap();
    assert_eq!(counts, (1, 1));
    // Memory resume may keep the same generation and last activity. A new
    // ready deadline still represents a new, independently idempotent cycle.
    let resumed = now + 600_000;
    sqlx::query("UPDATE api_cloud_workspaces SET state='ready',desired_state='ready',request_config=$1,next_action_at=$2 WHERE workspace_id='ws-bob'").bind(config()).bind(resumed).execute(&pool).await.unwrap();
    assert!(control.idle_sleep_once(resumed + grace).await.unwrap());
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM zuse_rust_lifecycle_operations")
            .fetch_one(&pool)
            .await
            .unwrap(),
        2
    );
    // Credit exhaustion must pause active work, regardless of recent activity.
    sqlx::raw_sql("CREATE TABLE zuse_brokeboy_compute_charges(probe text); CREATE TABLE api_cloud_billing_periods(account_id text,period_start bigint,period_end bigint,shared_credit_account boolean,shared_credit_hold boolean); INSERT INTO api_cloud_billing_periods VALUES('user-bob',0,999999999,true,false);").execute(&pool).await.unwrap();
    sqlx::query("UPDATE api_cloud_workspaces SET state='ready',desired_state='ready',request_config=$1,last_activity_at=$2,next_action_at=$2 WHERE workspace_id='ws-bob'").bind(config()).bind(resumed+grace).execute(&pool).await.unwrap();
    sqlx::query("UPDATE zuse_rust_task_command_execution SET state='running'")
        .execute(&pool)
        .await
        .unwrap();
    assert!(!control.idle_sleep_once(resumed + grace).await.unwrap());
    sqlx::query("UPDATE api_cloud_billing_periods SET shared_credit_hold=true")
        .execute(&pool)
        .await
        .unwrap();
    assert!(control.idle_sleep_once(resumed + grace).await.unwrap());
    let reason:String=sqlx::query_scalar("SELECT request_config->'rustLifecycleOperation'->>'reason' FROM api_cloud_workspaces WHERE workspace_id='ws-bob'").fetch_one(&pool).await.unwrap();
    assert_eq!(reason, "credits-exhausted");
    pool.close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .unwrap();
    admin.close().await;
}
