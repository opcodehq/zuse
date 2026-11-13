//! SQL atomicity fixture only: this does not certify a managed guest/base.
use super::*;
#[tokio::test]
#[ignore = "requires explicit isolated localhost PostgreSQL"]
async fn v2_initial_write_is_atomic_under_the_actual_restricted_role() {
    let url = std::env::var("ZUSE_CLOUD_TEST_DATABASE_URL").expect("isolated PostgreSQL required");
    let parsed = reqwest::Url::parse(&url).unwrap();
    assert!(matches!(
        parsed.host_str(),
        Some("127.0.0.1" | "localhost" | "::1")
    ));
    let admin = PgPool::connect(&url).await.unwrap();
    let schema = "zuse_staging".to_owned();
    let _staging = crate::STAGING_SCHEMA_LOCK.lock().await;
    sqlx::query(&format!("DROP SCHEMA IF EXISTS {schema} CASCADE"))
        .execute(&admin)
        .await
        .unwrap();
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
    for migration in [
        include_str!("../../../migrations/0000_application.sql"),
        include_str!("../../../migrations/0001_control_plane.sql"),
        include_str!("../../../migrations/0002_gateway_leases.sql"),
        include_str!("../../../migrations/0003_command_mailbox.sql"),
        include_str!("../../../migrations/0004_allocations.sql"),
        include_str!("../../../migrations/0005_lifecycle.sql"),
        include_str!("../../../migrations/0006_recovery.sql"),
        include_str!("../../../migrations/0007_images.sql"),
        include_str!("../../../migrations/0007_restore.sql"),
        include_str!("../../../migrations/0008_delete.sql"),
        include_str!("../../../migrations/0008_launch_assets.sql"),
        include_str!("../../../migrations/0009_computers.sql"),
        include_str!("../../../migrations/0010_managed_bases.sql"),
        include_str!("../../../migrations/0011_transcript_heads.sql"),
        include_str!("../../../migrations/0012_provider_receipts.sql"),
        include_str!("../../../migrations/0013_recovery_receipts.sql"),
        include_str!("../../../migrations/0014_restore_receipts.sql"),
        include_str!("../../../migrations/0015_github.sql"),
        include_str!("../../../migrations/0016_task_history.sql"),
        include_str!("../../../migrations/0017_account_events.sql"),
        include_str!("../../../migrations/0019_workload_secrets.sql"),
        include_str!("../../../migrations/0020_staging_managed_base_candidates.sql"),
        include_str!("../../../migrations/0023_managed_base_admission_lock.sql"),
        include_str!("../../../migrations/0025_admission_read_batch.sql"),
        include_str!("../../../migrations/0026_task_command_admission.sql"),
        include_str!("../../../migrations/0034_runtime_releases.sql"),
    ] {
        sqlx::raw_sql(migration).execute(&pool).await.unwrap();
    }
    // Use the actual provisioner role policy, including forbidden registry UPDATE.
    let database: String = sqlx::query_scalar("SELECT current_database()")
        .fetch_one(&admin)
        .await
        .unwrap();
    let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../infra/cloud/single-host/provision.py");
    let output=std::process::Command::new("python3").args(["-c","import importlib.util,sys;s=importlib.util.spec_from_file_location('provision',sys.argv[1]);m=importlib.util.module_from_spec(s);s.loader.exec_module(m);print(m.role_sql('staging',sys.argv[2],'synthetic-locked-role-password',True))"]).arg(script).arg(&database).output().unwrap();
    assert!(output.status.success());
    sqlx::raw_sql(std::str::from_utf8(&output.stdout).unwrap())
        .execute(&admin)
        .await
        .unwrap();
    let mut restricted_url = reqwest::Url::parse(&url).unwrap();
    restricted_url.set_username("zuse_cloud_staging").unwrap();
    restricted_url
        .set_password(Some("synthetic-locked-role-password"))
        .unwrap();
    let application = PgPool::connect(restricted_url.as_str()).await.unwrap();
    assert!(
        !sqlx::query_scalar::<_, bool>(
            "SELECT has_table_privilege(current_user,'cloud_managed_bases','UPDATE')"
        )
        .fetch_one(&application)
        .await
        .unwrap()
    );
    let denied = sqlx::query("SELECT provider FROM cloud_managed_bases WHERE false FOR SHARE")
        .execute(&application)
        .await
        .unwrap_err();
    assert_eq!(
        denied.as_database_error().unwrap().code().as_deref(),
        Some("42501")
    );
    let now = chrono::Utc::now().timestamp_millis();
    sqlx::query("INSERT INTO api_entitlements(entitlement_id,account_id,kind,provider,status,paid_through,created_at,updated_at) VALUES('ent','user-test','cloud-workspace','polar','active',$1,$2,$2)").bind(now+86400000).bind(now).execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO api_cloud_billing_periods(period_id,account_id,status,period_start,period_end,base_price_micros,included_provider_cost_micros,markup_basis_points,price_catalog_version,overage_cap_micros,created_at,updated_at) VALUES('period','user-test','manual',$1,$2,0,5000000,0,'fixture',0,$1,$1)").bind(now-1000).bind(now+86400000).execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO api_cloud_projects(project_id,account_id,repository_identity,repository_url,display_name,default_branch,visibility,git_connection_kind,configuration_digest,state,idempotency_key,created_at,updated_at) VALUES('project','user-test','github.com/example/repo','https://github.com/example/repo.git','repo','main','public','github-app','fixture','connected','project-key',$1,$1)").bind(now).execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO cloud_agent_connections(connection_id,owner_id,idempotency_key,request_digest,agent,method,state,created_at,updated_at) VALUES('connection','user-test','auth-key','fixture','codex','api-key','connected',$1,$1)").bind(now).execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO api_cloud_project_builds(build_id,project_id,account_id,provider,template_version,configuration_digest,state,snapshot_id,settings,idempotency_key,next_action_at,created_at,updated_at) VALUES('legacy-image','project','user-test','boxd','fixture','fixture','ready','fixture-snapshot','{}','legacy-key',0,$1,$1)").bind(now).execute(&pool).await.unwrap();
    let plan = |id: &str, command: bool| {
        json!({
            "workspaceId":id,"ownerId":"user-test","projectId":"project","buildId":"legacy-image","provider":"boxd","chatId":format!("chat:{id}"),"sessionId":format!("session:{id}"),"branch":format!("task/{id}"),"baseRef":"main","wrappedTranscriptKey":"fixture-ciphertext","idempotencyKey":format!("key:{id}"),"requestConfig":{"runtimeGeneration":1,"gatewayEpoch":1,"commandDeliveryMode":"server-admitted-v2","rustCommandAdmissionVersion":2},"bootTokenHash":"fixture-hash","bootExpiresAt":now+60000,"commandId":format!("launch:{id}"),"turnId":format!("turn:{id}"),"launchCiphertext":"fixture-ciphertext","launchExpiresAt":now+60000,"admissionCiphertext":"fixture-ciphertext","operationId":format!("allocate:{id}"),"snapshotId":"fixture-snapshot","machineName":format!("machine-{id}"),"storageIncarnationId":format!("disk:{id}"),"sealedBootToken":"fixture-ciphertext","jobId":format!("job:{id}"),"now":now,
            "initialTaskCommand":{"policyCiphertext":"fixture-ciphertext","sessionCiphertext":"fixture-ciphertext","actorId":"user-test","command":if command {json!({"requestDigest":"fixture-request-digest","envelope":{"fingerprint":"fixture-fingerprint","ciphertext":"fixture-ciphertext"},"envelopeBytes":128,"presentationCiphertext":"fixture-ciphertext","acceptance":{"commandId":format!("launch:{id}"),"durablyAdmitted":true}})}else{Value::Null}}
        })
    };
    let committed = plan("atomic", true);
    let mut tx = application.begin().await.unwrap();
    let created: Value = sqlx::query_scalar(WRITE_V2.as_str())
        .bind(&committed)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(created["workspace_id"], "atomic");
    // The first message, policy, registry, receipt and execution are visible only
    // with their workspace/allocation, and commit as a single statement/transaction.
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM api_cloud_workspaces WHERE workspace_id='atomic'"
        )
        .fetch_one(&pool)
        .await
        .unwrap(),
        0
    );
    tx.commit().await.unwrap();
    let counts:Value=sqlx::query_scalar("SELECT jsonb_build_object('workspace',(SELECT count(*) FROM api_cloud_workspaces WHERE workspace_id='atomic'),'history',(SELECT count(*) FROM zuse_rust_task_admissions WHERE workspace_id='atomic'),'session',(SELECT count(*) FROM zuse_rust_task_command_sessions WHERE workspace_id='atomic'),'action',(SELECT count(*) FROM zuse_rust_task_command_actions WHERE workspace_id='atomic'),'mailbox',(SELECT count(*) FROM zuse_rust_mailbox_commands WHERE workspace_id='atomic'),'execution',(SELECT count(*) FROM zuse_rust_task_command_execution WHERE workspace_id='atomic'),'allocation',(SELECT count(*) FROM zuse_rust_allocations WHERE workspace_id='atomic'),'job',(SELECT count(*) FROM zuse_rust_jobs WHERE resource_id='atomic'))").fetch_one(&application).await.unwrap();
    for count in counts.as_object().unwrap().values() {
        assert_eq!(count, 1);
    }
    let action: Value = sqlx::query_scalar(
        "SELECT acceptance FROM zuse_rust_task_command_actions WHERE workspace_id='atomic'",
    )
    .fetch_one(&application)
    .await
    .unwrap();
    assert_eq!(action["commandId"], "launch:atomic");
    assert_eq!(action["durablyAdmitted"], true);
    let duplicate = sqlx::query_scalar::<_, Value>(WRITE_V2.as_str())
        .bind(&committed)
        .fetch_one(&application)
        .await
        .unwrap_err();
    assert_eq!(
        duplicate.as_database_error().unwrap().code().as_deref(),
        Some("23505")
    );
    // Failure in the final execution record must roll back the preceding CTEs.
    let mut broken = plan("broken", true);
    broken["initialTaskCommand"]["command"]["presentationCiphertext"] = Value::Null;
    let failure = sqlx::query_scalar::<_, Value>(WRITE_V2.as_str())
        .bind(broken)
        .fetch_one(&application)
        .await
        .unwrap_err();
    let failure = failure.as_database_error().unwrap();
    assert_eq!(failure.code().as_deref(), Some("23502"));
    assert_eq!(failure.constraint(), None);
    for table in [
        "api_cloud_workspaces",
        "api_cloud_workspace_launch_intents",
        "zuse_rust_task_admissions",
        "zuse_rust_task_command_policies",
        "zuse_rust_task_command_sessions",
        "zuse_rust_mailboxes",
        "zuse_rust_mailbox_commands",
        "zuse_rust_task_command_actions",
        "zuse_rust_task_command_execution",
        "zuse_rust_allocations",
    ] {
        assert_eq!(
            sqlx::query_scalar::<_, i64>(&format!(
                "SELECT count(*) FROM {table} WHERE workspace_id='broken'"
            ))
            .fetch_one(&pool)
            .await
            .unwrap(),
            0,
            "partial row in {table}"
        );
    }
    let empty = plan("empty", false);
    sqlx::query_scalar::<_, Value>(WRITE_V2.as_str())
        .bind(empty)
        .fetch_one(&application)
        .await
        .unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM zuse_rust_task_command_sessions WHERE workspace_id='empty'"
        )
        .fetch_one(&application)
        .await
        .unwrap(),
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM zuse_rust_mailbox_commands WHERE workspace_id='empty'"
        )
        .fetch_one(&application)
        .await
        .unwrap(),
        0
    );
    use base64::Engine as _;
    // A second fixture path exercises real preparation/encryption and a fresh
    // device's API presentation without claiming a certified managed base.
    let master = crate::crypto::MasterKey::parse(
        &base64::engine::general_purpose::URL_SAFE_NO_PAD.encode([48u8; 32]),
    )
    .unwrap();
    let signing = ed25519_dalek::SigningKey::from_bytes(&[49; 32]);
    let jwk = json!({"kty":"OKP","crv":"Ed25519","d":base64::engine::general_purpose::URL_SAFE_NO_PAD.encode([49u8;32]),"x":base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(signing.verifying_key().as_bytes())});
    let control = std::sync::Arc::new(
        RuntimeControl::from_pool(
            application.clone(),
            crate::crypto::MasterKey::parse(
                &base64::engine::general_purpose::URL_SAFE_NO_PAD.encode([48u8; 32]),
            )
            .unwrap(),
            crate::tickets::TicketSigner::from_jwk(&jwk.to_string(), "https://api.example.test")
                .unwrap(),
        )
        .with_task_command_creation(true),
    );
    let access = crate::access::Access {
        actor: crate::auth::Principal {
            account_id: "user-test".into(),
        },
        scope: crate::access::Scope::Personal,
        owner_id: "user-test".into(),
        membership: None,
    };
    let (raw, wrapped) = master.create_transcript_key("user-test", "crypto").unwrap();
    let mut actual = plan("crypto", true);
    actual["wrappedTranscriptKey"] = json!(wrapped);
    actual["requestConfig"]["runtimeRpcProtocol"] = json!(zuse_protocol::cloud_gateway::RUST_RPC);
    actual["requestConfig"]["rustCommandMailboxEnabled"] = json!(true);
    actual["requestConfig"]["agent"] = json!("codex");
    actual["requestConfig"]["agentConnectionId"] = json!("connection");
    actual["requestConfig"]["agentConnectionOwnerId"] = json!("user-test");
    let row = json!({"workspace_id":"crypto","account_id":"user-test","chat_id":"chat:crypto","initial_session_id":"session:crypto","wrapped_transcript_key":wrapped,"state":"provisioning","desired_state":"ready","request_config":actual["requestConfig"]});
    let intent = json!({"workspaceId":"crypto","chatId":"chat:crypto","sessionId":"session:crypto","commandId":"launch:crypto","turnId":"turn:crypto","runtimeMode":"approval-required","model":"fixture","reasoning":null,"modelOptions":{},"firstMessage":"Real encrypted initial user message","title":"Fixture task","attachments":[]});
    let release = json!({"harnesses":[{"agent":"codex","catalog":{"id":"codex","name":"Codex","defaultModel":"fixture","models":[{"id":"fixture","label":"Fixture"}]}}]});
    let initial = control
        .prepare_initial_task_command(&row, &access, &intent, &release, now)
        .unwrap();
    actual["initialTaskCommand"] = serde_json::to_value(initial).unwrap();
    actual["admissionCiphertext"] = json!(
        master
            .seal_api("task-admission:user-test:crypto", &intent.to_string())
            .unwrap()
    );
    sqlx::query_scalar::<_, Value>(WRITE_V2.as_str())
        .bind(actual)
        .fetch_one(&application)
        .await
        .unwrap();
    let envelope: Value = sqlx::query_scalar(
        "SELECT envelope FROM zuse_rust_mailbox_commands WHERE workspace_id='crypto'",
    )
    .fetch_one(&application)
    .await
    .unwrap();
    let envelope: zuse_protocol::cloud_commands::CommandEnvelope =
        serde_json::from_value(envelope).unwrap();
    let key = zuse_cloud_runtime::mailbox::CommandKey::parse(&raw).unwrap();
    let opened = key.decrypt(&envelope).unwrap();
    let command: zuse_protocol::cloud_commands::CloudSessionCommand =
        serde_json::from_slice(&opened).unwrap();
    assert_eq!(command.command_id, "launch:crypto");
    assert_eq!(command.session_id, "session:crypto");
    let history = control
        .task_history(&access, "crypto", "session:crypto", None)
        .await
        .unwrap();
    assert_eq!(history["source"], "admission");
    assert_eq!(history["commandAdmissions"][0]["messageId"], "turn:crypto");
    assert_eq!(
        history["commandAdmissions"][0]["text"],
        "Real encrypted initial user message"
    );
    assert_eq!(history["session"]["chat"]["id"], "session:crypto");
    assert_eq!(
        history["registeredConversations"][0]["id"],
        "session:crypto"
    );

    assert!(control.task_command_creation_available().await.unwrap());
    let mut rollback = std::sync::Arc::try_unwrap(control)
        .ok()
        .expect("fixture control has no outstanding dispatch");
    rollback.creation_accounts.push("user-test".into());
    let control = std::sync::Arc::new(rollback.with_task_command_creation(false));
    assert!(!control.task_command_creation_available().await.unwrap());
    let request:super::super::creation::CreateWorkspace=serde_json::from_value(json!({"projectId":"project","providerId":"boxd","baseRef":"main","agent":"codex","model":"fixture","idempotencyKey":"new-after-creation-rollback","firstMessage":"Must not allocate","initialMessageDelivery":"server-admitted-v2"})).unwrap();
    assert_eq!(
        control
            .create_workspace(&access, request, now)
            .await
            .unwrap_err()
            .code,
        "cloud_task_command_admission_unavailable"
    );
    let leaked:i64=sqlx::query_scalar("SELECT count(*) FROM api_cloud_workspaces WHERE idempotency_key='new-after-creation-rollback'").fetch_one(&application).await.unwrap();
    assert_eq!(leaked, 0);
    // All queue/direct-dispatch assertions now run with creation disabled.
    queue_drain::exercise(&control, &application, &access, now).await;
    agent_switch::exercise(
        &control,
        &application,
        &access,
        &master,
        plan("agent-switch", false),
        now,
    )
    .await;
    runtime_outdated::exercise(
        &control,
        &application,
        &access,
        &master,
        |id| plan(id, false),
        now,
    )
    .await;
    let updating = std::sync::Arc::new(
        RuntimeControl::from_pool(
            application.clone(),
            crate::crypto::MasterKey::parse(
                &base64::engine::general_purpose::URL_SAFE_NO_PAD.encode([48u8; 32]),
            )
            .unwrap(),
            crate::tickets::TicketSigner::from_jwk(&jwk.to_string(), "https://api.example.test")
                .unwrap(),
        )
        .with_runtime_releases(super::super::runtime_updates::RuntimeReleaseConfig {
            channel: crate::runtime_releases::Channel::Stable,
            links: Some(std::sync::Arc::new(
                crate::objects::ArtifactLinks::new(
                    "https://account.r2.example.test/",
                    "zuse-runtime",
                    "access",
                    "fixture-secret",
                )
                .unwrap(),
            )),
        }),
    );
    runtime_updates::exercise(
        &updating,
        &pool,
        &application,
        &access,
        &master,
        |id| plan(id, false),
        now,
    )
    .await;
    // A partially installed optional schema must fail before acknowledging
    // any new queue/control mutation, even when all table grants are present.
    sqlx::query("ALTER TABLE zuse_rust_task_command_execution RENAME COLUMN priority TO incomplete_priority").execute(&pool).await.unwrap();
    let partial = RuntimeControl::from_pool(
        application.clone(),
        crate::crypto::MasterKey::parse(
            &base64::engine::general_purpose::URL_SAFE_NO_PAD.encode([48u8; 32]),
        )
        .unwrap(),
        crate::tickets::TicketSigner::from_jwk(&jwk.to_string(), "https://api.example.test")
            .unwrap(),
    );
    let request = zuse_protocol::cloud_task_commands::Request {
        command_id: "partial-schema-action".into(),
        session_id: "session:crypto".into(),
        action: zuse_protocol::cloud_task_commands::Action::PauseQueue { paused: false },
        wake: zuse_protocol::cloud_task_commands::WakePolicy::PreserveSleep,
    };
    assert_eq!(
        partial
            .task_command_submit("crypto", &access, request, now)
            .await
            .unwrap_err()
            .code,
        "cloud_task_command_schema_not_ready"
    );
    let accepted:i64=sqlx::query_scalar("SELECT count(*) FROM zuse_rust_task_command_actions WHERE command_id='partial-schema-action'").fetch_one(&application).await.unwrap();
    assert_eq!(accepted, 0);
    sqlx::query("ALTER TABLE zuse_rust_task_command_execution RENAME COLUMN incomplete_priority TO priority").execute(&pool).await.unwrap();
    application.close().await;
    pool.close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .unwrap();
    sqlx::query("DROP OWNED BY zuse_cloud_staging")
        .execute(&admin)
        .await
        .unwrap();
    sqlx::query("DROP ROLE zuse_cloud_staging")
        .execute(&admin)
        .await
        .unwrap();
}

mod object_io;

mod queue_drain;

mod interrupts;

mod agent_switch;

mod runtime_outdated;

mod asset_release;
mod runtime_updates;
