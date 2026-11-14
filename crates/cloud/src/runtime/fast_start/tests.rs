use super::*;
use crate::providers::{ExecResult, Machine, ProviderError, contract::*};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD as B64};
use std::sync::atomic::{AtomicUsize, Ordering};
struct BlockingAllocation {
    entered: AtomicUsize,
    release: tokio::sync::Notify,
}
#[async_trait::async_trait]
impl Provider for BlockingAllocation {
    fn id(&self) -> zuse_protocol::cloud_computers::ProviderId {
        zuse_protocol::cloud_computers::ProviderId::Boxd
    }
    async fn allocate(&self, _: &Allocation<'_>) -> Result<Machine, ProviderError> {
        let release = self.release.notified();
        tokio::pin!(release);
        release.as_mut().enable();
        self.entered.fetch_add(1, Ordering::SeqCst);
        release.await;
        Err(ProviderError::Rejected)
    }
    async fn find_allocation(&self, _: &str) -> Result<Option<Machine>, ProviderError> {
        Ok(None)
    }
    async fn inspect(&self, _: &str) -> Result<Option<Machine>, ProviderError> {
        unreachable!()
    }
    async fn sleep(&self, _: &str) -> Result<ProviderOperation, ProviderError> {
        unreachable!()
    }
    async fn resume(&self, _: &str, _: u32) -> Result<ProviderOperation, ProviderError> {
        unreachable!()
    }
    async fn delete(&self, _: &str) -> Result<ProviderOperation, ProviderError> {
        unreachable!()
    }
    async fn operation(&self, _: &ProviderOperation) -> Result<OperationState, ProviderError> {
        unreachable!()
    }
    async fn execute(&self, _: &str, _: &str) -> Result<ExecResult, ProviderError> {
        unreachable!()
    }
    async fn upload(&self, _: &str, _: &str, _: &[u8]) -> Result<(), ProviderError> {
        unreachable!()
    }
}

#[tokio::test]
#[ignore = "requires explicit isolated localhost PostgreSQL"]
async fn gateway_ack_and_early_readiness_preserve_checkout_and_initial_command_fences() {
    let url = std::env::var("ZUSE_CLOUD_TEST_DATABASE_URL").unwrap();
    assert!(matches!(
        reqwest::Url::parse(&url).unwrap().host_str(),
        Some("127.0.0.1" | "localhost" | "::1")
    ));
    let admin = PgPool::connect(&url).await.unwrap();
    let schema = format!("fast_start_{}", uuid::Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&admin)
        .await
        .unwrap();
    let scope = schema.clone();
    let pool = PgPoolOptions::new()
        .max_connections(4)
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
        crate::runtime::mailbox::MIGRATION,
    ] {
        sqlx::raw_sql(sql).execute(&pool).await.unwrap();
    }
    let signing = ed25519_dalek::SigningKey::from_bytes(&[84; 32]);
    let jwk = json!({"kty":"OKP","crv":"Ed25519","d":B64.encode([84;32]),"x":B64.encode(signing.verifying_key().as_bytes())});
    let control = RuntimeControl::from_pool(
        pool.clone(),
        MasterKey::parse(&B64.encode([83; 32])).unwrap(),
        TicketSigner::from_jwk(&jwk.to_string(), "https://api.fixture.test").unwrap(),
    );
    let now = chrono::Utc::now().timestamp_millis();
    let config = json!({"runtimeGeneration":1,"gatewayEpoch":1,"rustFastStartVersion":1,
        "runtimeRpcProtocol":zuse_protocol::cloud_gateway::RUST_RPC,"runtimeCredentialExpiresAtMs":now+600000,
        "runtimeBootstrapReceipt":{"workspaceId":"ws-bob","generation":1,"gatewayEpoch":1,"capabilities":["rust-fast-start-v1","rust-session-commands-v1"]},"rustCommandMailboxEnabled":true,
        "runtimeCredentialKeyThumbprint":"credential","runtimeSigningKeyThumbprint":"signing",
        "rustRuntimeStorage":{"storageIncarnationId":"disk"},
        "rustStorageEnrollment":{"phase":"committed","generation":1,"sourceSandboxId":"machine-bob","storageIncarnationId":"disk","credentialKeyThumbprint":"credential","signingKeyThumbprint":"signing"},
        "rustAllocationOperationId":"allocation","rustPreparationIngress":{"operationId":"allocation","generation":1,"sourceMachineId":"machine-bob","storageIncarnationId":"disk"},
        "startupTimings":{"requestedAt":now},"preparation":{"repository":{"state":"running"}}});
    sqlx::query("UPDATE api_cloud_workspaces SET request_config=$1,state='setup',runtime_state='connecting',runtime_credential_hash=$2,runtime_boot_token_hash='one-shot-boot',runtime_boot_token_expires_at=$3 WHERE workspace_id='ws-bob'")
        .bind(config).bind(hash("runtime-fixture")).bind(now+600000).execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO zuse_rust_control_owners VALUES('workspace','ws-bob','rust',1,$1)")
        .bind(now)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO api_cloud_workspace_launch_intents VALUES('ws-bob','user-bob','chat-bob','session-bob','turn','launch:ws-bob','fixture-ciphertext',$1,$2)").bind(now+600000).bind(now).execute(&pool).await.unwrap();
    assert!(
        control
            .gateway_authorize(
                "ws-bob",
                zuse_protocol::cloud_gateway::RUST_PROTOCOL,
                "wrong",
                now
            )
            .await
            .is_err()
    );
    let grant = control
        .gateway_authorize(
            "ws-bob",
            zuse_protocol::cloud_gateway::RUST_PROTOCOL,
            "runtime-fixture",
            now,
        )
        .await
        .unwrap();
    assert!(grant.runtime);
    control
        .gateway_authorize(
            "ws-bob",
            zuse_protocol::cloud_gateway::RUST_PROTOCOL,
            "runtime-fixture",
            now + 1,
        )
        .await
        .unwrap();
    let row: Value = sqlx::query_scalar(
        "SELECT to_jsonb(w) FROM api_cloud_workspaces w WHERE workspace_id='ws-bob'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(row["runtime_boot_token_hash"].is_null());
    assert_eq!(
        row["request_config"]["runtimeBootstrapReceipt"]["acknowledgedAtMs"],
        now
    );
    assert_eq!(
        row["request_config"]["startupTimings"]["gatewayAuthorizedAt"],
        now
    );
    assert_eq!(row["state"], "setup");
    let request = || zuse_protocol::cloud::CloudRuntimeReady {
        phase: "agent-ready".into(),
        launch_command_id: Some("launch:ws-bob".into()),
        session_head_version: Some(0),
    };
    control
        .ready_launch("ws-bob", "runtime-fixture", request(), now + 2)
        .await
        .unwrap();
    let row: Value = sqlx::query_scalar(
        "SELECT to_jsonb(w) FROM api_cloud_workspaces w WHERE workspace_id='ws-bob'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(RuntimeControl::early_agent_ready(&row));
    assert_eq!(row["state"], "setup");
    assert!(row["request_config"]["startupTimings"]["repositoryReadyAt"].is_null());
    let intents: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM api_cloud_workspace_launch_intents WHERE workspace_id='ws-bob'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        intents, 1,
        "early readiness must not retire or duplicate the initial command"
    );
    // The task's first message is a server-admitted mailbox command. An early
    // agent leases and accepts it while the checkout still runs, and the
    // public timing says so instead of waiting for repository readiness.
    let envelope = zuse_protocol::cloud_commands::CommandEnvelope {
        protocol_version: zuse_protocol::cloud_commands::PROTOCOL_VERSION,
        workspace_id: "ws-bob".into(),
        session_id: "session-bob".into(),
        command_id: "launch:ws-bob".into(),
        kind: "zuse.session.run".into(),
        fingerprint: format!("hmac-sha256:{}", "a".repeat(43)),
        schema_version: 1,
        key_version: 1,
        destruction_fence: 0,
        created_at: now,
        iv: "iv".into(),
        ciphertext: "ciphertext".into(),
        dependencies: vec![],
        actor: None,
    };
    sqlx::query(
        "INSERT INTO zuse_rust_mailboxes(workspace_id,sequence,revision) VALUES('ws-bob',1,1)",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO zuse_rust_mailbox_commands(workspace_id,command_id,workspace_sequence,revision,lane,fingerprint,envelope,envelope_bytes,state,accepted_at,updated_at) VALUES('ws-bob','launch:ws-bob',1,1,'task-execution:session-bob',$1,$2,1,'waiting-for-runtime',$3,$3)")
        .bind(&envelope.fingerprint).bind(serde_json::to_value(&envelope).unwrap()).bind(now).execute(&pool).await.unwrap();
    let page = control
        .command_lease(
            "ws-bob",
            "runtime-fixture",
            &zuse_protocol::cloud_commands::LeaseRequest {
                control_only: false,
                max_items: None,
                storage_incarnation_id: "disk".into(),
                wait_ms: None,
                after_revision: None,
            },
            now + 3,
        )
        .await
        .unwrap();
    assert_eq!(
        page.leases.len(),
        1,
        "an early agent leases the first message before the checkout is ready"
    );
    let ack: zuse_protocol::cloud_commands::RuntimeAcknowledgment = serde_json::from_value(
        json!({"commandId":"launch:ws-bob","leaseToken":page.leases[0].lease_token,"fingerprint":envelope.fingerprint,"state":"applied"}),
    )
    .unwrap();
    control
        .command_ack("ws-bob", "runtime-fixture", &ack, now + 4)
        .await
        .unwrap();
    let accepted: Value = sqlx::query_scalar(
        "SELECT to_jsonb(w) FROM api_cloud_workspaces w WHERE workspace_id='ws-bob'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let timings = &accepted["request_config"]["startupTimings"];
    assert_eq!(timings["messageAcceptedAt"], now + 4);
    assert!(timings["repositoryReadyAt"].is_null());
    assert_eq!(accepted["state"], "setup");
    let public = crate::store::Workspace {
        row: accepted.clone(),
    }
    .public();
    assert_eq!(public["startupTimings"]["agentReadyAt"], now + 2);
    assert_eq!(public["startupTimings"]["messageAcceptedAt"], now + 4);
    // Repository readiness completes the launch later without moving the
    // earlier acceptance.
    control
        .ready_launch(
            "ws-bob",
            "runtime-fixture",
            zuse_protocol::cloud::CloudRuntimeReady {
                phase: "agent-started".into(),
                launch_command_id: Some("launch:ws-bob".into()),
                session_head_version: Some(1),
            },
            now + 9,
        )
        .await
        .unwrap();
    let completed: Value = sqlx::query_scalar(
        "SELECT request_config->'startupTimings' FROM api_cloud_workspaces WHERE workspace_id='ws-bob'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(completed["repositoryReadyAt"], now + 9);
    assert_eq!(completed["messageAcceptedAt"], now + 4);
    let mut failed = row.clone();
    failed["request_config"]["preparation"]["repository"]["state"] = json!("failed");
    assert!(!RuntimeControl::early_agent_ready(&failed));
    sqlx::query("UPDATE zuse_rust_control_owners SET generation=2 WHERE resource_id='ws-bob'")
        .execute(&pool)
        .await
        .unwrap();
    assert!(
        control
            .ready_launch("ws-bob", "runtime-fixture", request(), now + 3)
            .await
            .is_err()
    );
    // Two blocked provider calls must not serialize each other, hold workspace
    // locks, or let a third committed computer exceed the concurrency bound.
    sqlx::raw_sql(crate::runtime::allocation::MIGRATION)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::raw_sql(crate::runtime::deletion::MIGRATION)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO api_cloud_workspaces SELECT (jsonb_populate_record(NULL::api_cloud_workspaces,to_jsonb(w)||'{\"workspace_id\":\"ws-third\"}'::jsonb)).* FROM api_cloud_workspaces w WHERE workspace_id='ws-bob'").execute(&pool).await.unwrap();
    for workspace in ["ws-bob", "ws-alice", "ws-third"] {
        sqlx::query("UPDATE api_cloud_workspaces SET provider='boxd',provider_sandbox_id=NULL,runtime_boot_token_hash=NULL,runtime_credential_hash=NULL,state='queued',runtime_state='offline',desired_state='ready',request_config=$2 WHERE workspace_id=$1")
            .bind(workspace).bind(json!({"runtimeGeneration":1,"gatewayEpoch":1,"rustFastStartVersion":1,"rustProviderControllerVersion":1})).execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO zuse_rust_control_owners VALUES('workspace',$1,'rust',1,$2) ON CONFLICT(resource_kind,resource_id) DO UPDATE SET generation=1").bind(workspace).bind(now).execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO api_cloud_workspace_launch_intents VALUES($1,'user-bob','chat','session','turn','command','ciphertext',$2,$3) ON CONFLICT(workspace_id) DO NOTHING").bind(workspace).bind(now+600000).bind(now).execute(&pool).await.unwrap();
        control
            .queue_allocation(workspace, "managed-artifact", now)
            .await
            .unwrap();
    }
    let provider = Arc::new(BlockingAllocation {
        entered: AtomicUsize::new(0),
        release: tokio::sync::Notify::new(),
    });
    let mut registry = crate::providers::registry::Registry::default();
    registry.insert(provider.clone()).unwrap();
    let cancel = tokio_util::sync::CancellationToken::new();
    let task = tokio::spawn(crate::runtime::allocation::workers::run(
        Arc::new(control),
        Arc::new(registry),
        2,
        Duration::from_secs(60),
        cancel.clone(),
    ));
    tokio::time::timeout(Duration::from_secs(5), async {
        while provider.entered.load(Ordering::SeqCst) != 2 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("two original allocations must reach the provider concurrently");
    let active: i64 =
        sqlx::query_scalar("SELECT count(*) FROM zuse_rust_jobs WHERE state='running'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(active, 2);
    tokio::time::timeout(Duration::from_secs(1), sqlx::query("UPDATE api_cloud_workspaces SET revision=revision+1 WHERE workspace_id IN ('ws-bob','ws-alice','ws-third')").execute(&pool)).await.expect("allocation effects must not hold workspace locks").unwrap();
    cancel.cancel();
    provider.release.notify_waiters();
    task.await.unwrap().unwrap();
    assert_eq!(
        provider.entered.load(Ordering::SeqCst),
        2,
        "shutdown must not allocate the queued third computer"
    );
    pool.close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .unwrap();
    admin.close().await;
}
