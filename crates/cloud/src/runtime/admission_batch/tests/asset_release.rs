//! Server-held attachments leave object storage once the computer has them,
//! and expire when never delivered. Real PG under the restricted role.
use super::*;
use base64::Engine as _;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use zuse_protocol::cloud_task_commands as w;

#[derive(Default)]
struct Memory(Mutex<BTreeMap<String, Vec<u8>>>);
impl Memory {
    fn keys(&self, asset: &str) -> usize {
        self.0
            .lock()
            .unwrap()
            .keys()
            .filter(|k| k.contains(asset))
            .count()
    }
}
#[async_trait::async_trait]
impl crate::objects::Objects for Memory {
    async fn get(&self, key: &str) -> Result<Option<Vec<u8>>, crate::error::ApiError> {
        Ok(self.0.lock().unwrap().get(key).cloned())
    }
    async fn put(&self, key: &str, bytes: &[u8]) -> Result<(), crate::error::ApiError> {
        let mut state = self.0.lock().unwrap();
        if state.get(key).is_some_and(|prior| prior != bytes) {
            return Err(crate::error::ApiError::conflict("fixture_immutable"));
        }
        state.insert(key.into(), bytes.to_vec());
        Ok(())
    }
    fn supports_purge(&self) -> bool {
        true
    }
    async fn purge_prefix(&self, prefix: &str) -> Result<bool, crate::error::ApiError> {
        self.0
            .lock()
            .unwrap()
            .retain(|key, _| !key.starts_with(prefix));
        Ok(true)
    }
    async fn delete_object(&self, key: &str) -> Result<(), crate::error::ApiError> {
        self.0.lock().unwrap().remove(key);
        Ok(())
    }
}

const WS: &str = "release-task";
const SESSION: &str = "session-release-task";
const DISK: &str = "disk:release-task";

fn access() -> crate::access::Access {
    crate::access::Access {
        actor: crate::auth::Principal {
            account_id: "user-test".into(),
        },
        scope: crate::access::Scope::Personal,
        owner_id: "user-test".into(),
        membership: None,
    }
}

async fn upload(control: &RuntimeControl, name: &str, now: i64) -> Value {
    let bytes = format!("private bytes of {name}").into_bytes();
    let b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD;
    control
        .workspace_asset_chunk(
            WS,
            &access(),
            zuse_protocol::cloud_assets::Chunk {
                idempotency_key: format!("workspace:{WS}"),
                upload_id: name.into(),
                seq: 0,
                bytes: b64.encode(&bytes),
            },
            now,
        )
        .await
        .unwrap();
    let committed = control
        .workspace_asset_commit(
            WS,
            &access(),
            zuse_protocol::cloud_assets::Commit {
                idempotency_key: format!("workspace:{WS}"),
                upload_id: name.into(),
                mime_type: "text/plain".into(),
                original_name: format!("{name}.txt"),
                size_bytes: bytes.len(),
                sha256: b64.encode(sha2::Sha256::digest(&bytes)),
            },
            now,
        )
        .await
        .unwrap();
    committed["asset"].clone()
}

fn run(cid: &str, assets: &[&Value]) -> w::Request {
    serde_json::from_value(json!({"commandId":cid,"sessionId":SESSION,"action":{"type":"submit","messageId":format!("m:{cid}"),"attachments":assets,"payload":{"kind":"run","messageId":format!("m:{cid}"),"request":{"prompt":"read the file","harness":"codex","model":"fixture","cwd":".","sandbox":"workspace-write"}}}})).unwrap()
}

/// Lease the next command, acknowledge delivery, and report these states.
async fn execute(control: &RuntimeControl, expected: &str, states: &[w::ExecutionState], now: i64) {
    let request = zuse_protocol::cloud_commands::LeaseRequest {
        control_only: false,
        max_items: None,
        storage_incarnation_id: DISK.into(),
        wait_ms: Some(0),
        after_revision: None,
    };
    let lease = control
        .command_lease(WS, "runtime-fixture", &request, now)
        .await
        .unwrap();
    assert_eq!(lease.leases.len(), 1, "expected {expected} to be leased");
    let lease = &lease.leases[0];
    assert_eq!(lease.command.command_id, expected);
    control
        .command_ack(
            WS,
            "runtime-fixture",
            &zuse_protocol::cloud_commands::RuntimeAcknowledgment {
                command_id: lease.command.command_id.clone(),
                lease_token: lease.lease_token.clone(),
                fingerprint: lease.command.fingerprint.clone(),
                state: zuse_protocol::cloud_commands::AcknowledgmentState::Applied,
                category: None,
                result_iv: None,
                result_ciphertext: None,
            },
            now,
        )
        .await
        .unwrap();
    for (index, state) in states.iter().enumerate() {
        report(
            control,
            expected,
            &lease.command.fingerprint,
            index as i64 + 1,
            *state,
            now,
        )
        .await;
    }
}

async fn report(
    control: &RuntimeControl,
    command: &str,
    fingerprint: &str,
    revision: i64,
    state: w::ExecutionState,
    now: i64,
) {
    control
        .task_command_progress(
            WS,
            "runtime-fixture",
            &w::Progress {
                command_id: command.into(),
                fingerprint: fingerprint.into(),
                storage_incarnation_id: DISK.into(),
                revision,
                state,
                turn_id: None,
                error_code: None,
            },
            now,
        )
        .await
        .unwrap();
}

async fn state(pool: &PgPool, asset: &Value) -> (String, Option<String>) {
    sqlx::query_as("SELECT state,release_reason FROM zuse_rust_launch_assets WHERE workspace_id=$1 AND asset_id=$2").bind(WS).bind(asset["assetId"].as_str().unwrap()).fetch_one(pool).await.unwrap()
}

#[tokio::test]
#[ignore = "requires explicit isolated localhost PostgreSQL"]
async fn delivered_attachments_leave_object_storage_and_undelivered_ones_expire() {
    use w::ExecutionState::{Completed, Failed, Running, Starting};
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
        include_str!("../../../../migrations/0000_application.sql"),
        include_str!("../../../../migrations/0001_control_plane.sql"),
        include_str!("../../../../migrations/0002_gateway_leases.sql"),
        include_str!("../../../../migrations/0003_command_mailbox.sql"),
        include_str!("../../../../migrations/0004_allocations.sql"),
        include_str!("../../../../migrations/0005_lifecycle.sql"),
        include_str!("../../../../migrations/0006_recovery.sql"),
        include_str!("../../../../migrations/0007_images.sql"),
        include_str!("../../../../migrations/0007_restore.sql"),
        include_str!("../../../../migrations/0008_delete.sql"),
        include_str!("../../../../migrations/0008_launch_assets.sql"),
        include_str!("../../../../migrations/0009_computers.sql"),
        include_str!("../../../../migrations/0010_managed_bases.sql"),
        include_str!("../../../../migrations/0011_transcript_heads.sql"),
        include_str!("../../../../migrations/0012_provider_receipts.sql"),
        include_str!("../../../../migrations/0013_recovery_receipts.sql"),
        include_str!("../../../../migrations/0014_restore_receipts.sql"),
        include_str!("../../../../migrations/0015_github.sql"),
        include_str!("../../../../migrations/0016_task_history.sql"),
        include_str!("../../../../migrations/0017_account_events.sql"),
        include_str!("../../../../migrations/0019_workload_secrets.sql"),
        include_str!("../../../../migrations/0020_staging_managed_base_candidates.sql"),
        include_str!("../../../../migrations/0023_managed_base_admission_lock.sql"),
        include_str!("../../../../migrations/0025_admission_read_batch.sql"),
        include_str!("../../../../migrations/0026_task_command_admission.sql"),
        include_str!("../../../../migrations/0034_runtime_releases.sql"),
        crate::runtime::assets::RELEASE_MIGRATION,
        // Re-running is harmless (the bootstrap applies every file each time).
        crate::runtime::assets::RELEASE_MIGRATION,
    ] {
        sqlx::raw_sql(migration).execute(&pool).await.unwrap();
    }
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
    let now = chrono::Utc::now().timestamp_millis();
    sqlx::query("INSERT INTO api_cloud_projects(project_id,account_id,repository_identity,repository_url,display_name,default_branch,visibility,git_connection_kind,configuration_digest,state,idempotency_key,created_at,updated_at) VALUES('project','user-test','github.com/example/repo','https://github.com/example/repo.git','repo','main','public','github-app','fixture','connected','project-key',$1,$1)").bind(now).execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO api_cloud_project_builds(build_id,project_id,account_id,provider,template_version,configuration_digest,state,snapshot_id,settings,idempotency_key,next_action_at,created_at,updated_at) VALUES('legacy-image','project','user-test','boxd','fixture','fixture','ready','fixture-snapshot','{}','legacy-key',0,$1,$1)").bind(now).execute(&pool).await.unwrap();
    let b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD;
    let master = crate::crypto::MasterKey::parse(&b64.encode([48u8; 32])).unwrap();
    let signing = ed25519_dalek::SigningKey::from_bytes(&[49; 32]);
    let jwk = json!({"kty":"OKP","crv":"Ed25519","d":b64.encode([49u8;32]),"x":b64.encode(signing.verifying_key().as_bytes())});
    let objects = Arc::new(Memory::default());
    let control = Arc::new(
        RuntimeControl::from_pool(
            application.clone(),
            crate::crypto::MasterKey::parse(&b64.encode([48u8; 32])).unwrap(),
            crate::tickets::TicketSigner::from_jwk(&jwk.to_string(), "https://api.example.test")
                .unwrap(),
        )
        .with_transcript_objects(objects.clone()),
    );
    let (_, wrapped) = master.create_transcript_key("user-test", WS).unwrap();
    let config = json!({"runtimeGeneration":1,"gatewayEpoch":1,"commandDeliveryMode":"server-admitted-v2","rustCommandAdmissionVersion":2,"runtimeRpcProtocol":zuse_protocol::cloud_gateway::RUST_RPC,"rustCommandMailboxEnabled":true,"agent":"codex","agentConnectionId":"connection","agentConnectionOwnerId":"user-test","rustRuntimeStorage":{"storageIncarnationId":DISK}});
    let row = json!({"workspace_id":WS,"account_id":"user-test","chat_id":"chat-release","initial_session_id":SESSION,"wrapped_transcript_key":wrapped,"state":"provisioning","desired_state":"ready","request_config":config});
    let intent = json!({"workspaceId":WS,"chatId":"chat-release","sessionId":SESSION,"commandId":format!("launch:{WS}"),"turnId":format!("turn:{WS}"),"runtimeMode":"approval-required","model":"fixture","reasoning":null,"modelOptions":{},"firstMessage":"Initial durable task","title":"Fixture","attachments":[]});
    let release = json!({"harnesses":[{"agent":"codex","catalog":{"id":"codex","name":"Codex","defaultModel":"fixture","models":[{"id":"fixture","label":"Fixture"}]}}]});
    let plan = json!({
        "workspaceId":WS,"ownerId":"user-test","projectId":"project","buildId":"legacy-image","provider":"boxd","chatId":"chat-release","sessionId":SESSION,"branch":"task/release","baseRef":"main","wrappedTranscriptKey":wrapped,"idempotencyKey":"key:release","requestConfig":config,"bootTokenHash":"fixture-hash","bootExpiresAt":now+60000,"commandId":format!("launch:{WS}"),"turnId":format!("turn:{WS}"),"launchCiphertext":"fixture-ciphertext","launchExpiresAt":now+60000,"admissionCiphertext":master.seal_api(&format!("task-admission:user-test:{WS}"),&intent.to_string()).unwrap(),"operationId":"allocate:release","snapshotId":"fixture-snapshot","machineName":"machine-release","storageIncarnationId":DISK,"sealedBootToken":"fixture-ciphertext","jobId":"job:release","now":now,
        "initialTaskCommand":control.prepare_initial_task_command(&row, &access(), &intent, &release, now).unwrap(),
    });
    sqlx::query_scalar::<_, Value>(WRITE_V2.as_str())
        .bind(plan)
        .fetch_one(&application)
        .await
        .unwrap();
    let enrollment = json!({"runtimeBootstrapReceipt":{"workspaceId":WS,"generation":1,"gatewayEpoch":1},"runtimeCredentialExpiresAtMs":now+3600000,"runtimeCredentialKeyThumbprint":"key","runtimeSigningKeyThumbprint":"sign","rustStorageEnrollment":{"phase":"committed","generation":1,"sourceSandboxId":"release-machine","storageIncarnationId":DISK,"credentialKeyThumbprint":"key","signingKeyThumbprint":"sign","capabilities":["rust-session-commands-v1",w::CAPABILITY]}});
    sqlx::query("UPDATE api_cloud_workspaces SET state='ready',desired_state='ready',runtime_state='online',provider_sandbox_id='release-machine',runtime_credential_hash=$2,request_config=request_config||$3 WHERE workspace_id=$1").bind(WS).bind(hash("runtime-fixture")).bind(enrollment).execute(&application).await.unwrap();
    execute(
        &control,
        &format!("launch:{WS}"),
        &[Starting, Completed],
        now,
    )
    .await;

    // Shared: bound by two commands. The first delivery must not delete it
    // while the second, not yet started, still has to fetch it.
    let shared = upload(&control, "shared", now).await;
    let shared_id = shared["assetId"].as_str().unwrap();
    assert_eq!(objects.keys(shared_id), 2, "one chunk plus its manifest");
    for cid in ["run:a", "run:b"] {
        let request = run(cid, &[&shared]);
        let accepted = control
            .task_command_submit(WS, &access(), request.clone(), now)
            .await
            .unwrap();
        // A retried submit is the same logical command and one binding.
        assert_eq!(
            control
                .task_command_submit(WS, &access(), request, now)
                .await
                .unwrap(),
            accepted
        );
    }
    let holds: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM zuse_rust_launch_asset_holds WHERE asset_id=$1 AND kind='command'",
    )
    .bind(shared_id)
    .fetch_one(&application)
    .await
    .unwrap();
    assert_eq!(holds, 2);
    execute(&control, "run:a", &[Starting, Running], now).await;
    assert_eq!(state(&application, &shared).await.0, "ready");
    assert_eq!(
        objects.keys(shared_id),
        2,
        "deleted while run:b still needs it"
    );
    let fingerprint: String = sqlx::query_scalar("SELECT fingerprint FROM zuse_rust_task_command_execution WHERE workspace_id=$1 AND command_id='run:a'").bind(WS).fetch_one(&application).await.unwrap();
    report(&control, "run:a", &fingerprint, 3, Completed, now).await;
    // Still held by run:b, which has not fetched it yet.
    assert!(!control.sweep_asset_releases_once(now).await.unwrap());
    assert_eq!(objects.keys(shared_id), 2);
    execute(&control, "run:b", &[Starting, Running], now).await;
    assert_eq!(
        state(&application, &shared).await,
        ("released".into(), Some("delivered".into()))
    );
    assert_eq!(
        objects.keys(shared_id),
        0,
        "delivered attachment left in object storage"
    );
    // Metadata only: no chunk rows or ciphertext remain for it.
    let chunks: i64 =
        sqlx::query_scalar("SELECT count(*) FROM zuse_rust_task_asset_chunks WHERE asset_id=$1")
            .bind(shared_id)
            .fetch_one(&application)
            .await
            .unwrap();
    assert_eq!(chunks, 0);
    // A released file cannot be bound again; the client uploads it anew.
    let error = control
        .task_command_submit(WS, &access(), run("run:again", &[&shared]), now)
        .await
        .unwrap_err();
    assert_eq!(error.code, "launch_asset_released");
    let fingerprint: String = sqlx::query_scalar("SELECT fingerprint FROM zuse_rust_task_command_execution WHERE workspace_id=$1 AND command_id='run:b'").bind(WS).fetch_one(&application).await.unwrap();
    report(&control, "run:b", &fingerprint, 3, Completed, now).await;

    // Refused before fetching: the command fails while starting. The file is
    // kept (it may be sent again) and only expiry removes it.
    let refused = upload(&control, "refused", now).await;
    let refused_id = refused["assetId"].as_str().unwrap();
    control
        .task_command_submit(WS, &access(), run("run:c", &[&refused]), now)
        .await
        .unwrap();
    execute(&control, "run:c", &[Starting, Failed], now).await;
    assert_eq!(state(&application, &refused).await.0, "ready");
    assert_eq!(objects.keys(refused_id), 2);

    // Queued but never sent: a delivered command does not release a file a
    // queued message still references; removing that message does.
    let queued = upload(&control, "queued", now).await;
    let queued_id = queued["assetId"].as_str().unwrap();
    let input: w::Input =
        serde_json::from_value(json!({"text":"later","attachments":[queued]})).unwrap();
    control
        .task_command_submit(
            WS,
            &access(),
            w::Request {
                command_id: "queue-later".into(),
                session_id: SESSION.into(),
                action: w::Action::Queue {
                    queue_id: "q-later".into(),
                    message_id: "m:q-later".into(),
                    input,
                    hold_for_turn_end: true,
                },
                wake: w::WakePolicy::PreserveSleep,
            },
            now,
        )
        .await
        .unwrap();
    control
        .task_command_submit(WS, &access(), run("run:d", &[&queued]), now)
        .await
        .unwrap();
    execute(&control, "run:d", &[Starting, Running, Completed], now).await;
    assert_eq!(state(&application, &queued).await.0, "ready");
    assert_eq!(objects.keys(queued_id), 2);
    sqlx::query("UPDATE zuse_rust_task_command_queue SET state='deleted' WHERE workspace_id=$1 AND queue_id='q-later'").bind(WS).execute(&application).await.unwrap();
    assert!(control.sweep_asset_releases_once(now).await.unwrap());
    assert_eq!(
        state(&application, &queued).await,
        ("released".into(), Some("delivered".into()))
    );
    assert_eq!(objects.keys(queued_id), 0);

    // Waiting for the computer: a message this runtime can't take yet (full
    // access) is queued with its server-held file, which stays in object
    // storage while it waits and leaves once the updated computer has it.
    let waiting = upload(&control, "waiting", now).await;
    let waiting_id = waiting["assetId"].as_str().unwrap();
    let request: w::Request = serde_json::from_value(json!({"commandId":"run:wait","sessionId":SESSION,"action":{"type":"submit","messageId":"m:run:wait","attachments":[waiting],"payload":{"kind":"run","messageId":"m:run:wait","request":{"prompt":"read the file","harness":"codex","model":"fixture","cwd":".","sandbox":"danger-full-access","autoApprove":true}}}})).unwrap();
    let accepted = control
        .task_command_submit_with(
            WS,
            &access(),
            request,
            crate::runtime::task_commands::Submit {
                queue_while_waiting: true,
            },
            now,
        )
        .await
        .unwrap();
    assert_eq!(
        accepted["waitingFor"],
        zuse_protocol::cloud_runtime_updates::RUNTIME_UPDATE_UNAVAILABLE
    );
    let held: Vec<(String, String)> =
        sqlx::query_as("SELECT kind,holder FROM zuse_rust_launch_asset_holds WHERE asset_id=$1")
            .bind(waiting_id)
            .fetch_all(&application)
            .await
            .unwrap();
    assert_eq!(held, [("queue".to_owned(), "m:run:wait".to_owned())]);
    assert!(control.work_task_drain_for(WS, now).await.unwrap());
    assert!(!control.sweep_asset_releases_once(now).await.unwrap());
    assert_eq!(objects.keys(waiting_id), 2, "kept while it waits");
    // Update computer finished on a runtime that takes per-message access.
    sqlx::query("UPDATE api_cloud_workspaces SET request_config=jsonb_set(request_config,'{runtimeBootstrapReceipt,capabilities}',$2) WHERE workspace_id=$1").bind(WS).bind(json!([w::MESSAGE_ACCESS_MODE_CAPABILITY])).execute(&pool).await.unwrap();
    let mut tx = application.begin().await.unwrap();
    let (row, _, _) = locked_workspace_with(&mut tx, WS, WorkspaceLock::Command)
        .await
        .unwrap();
    RuntimeControl::resume_waiting_queues_tx(&mut tx, &row, now)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert!(control.work_task_drain_for(WS, now).await.unwrap());
    let claimed: String = sqlx::query_scalar("SELECT claimed_command_id FROM zuse_rust_task_command_queue WHERE workspace_id=$1 AND queue_id='m:run:wait'").bind(WS).fetch_one(&application).await.unwrap();
    execute(&control, &claimed, &[Starting, Running], now).await;
    assert_eq!(
        state(&application, &waiting).await,
        ("released".into(), Some("delivered".into()))
    );
    assert_eq!(
        objects.keys(waiting_id),
        0,
        "left object storage once delivered"
    );
    let fingerprint: String = sqlx::query_scalar("SELECT fingerprint FROM zuse_rust_task_command_execution WHERE workspace_id=$1 AND command_id=$2").bind(WS).bind(&claimed).fetch_one(&application).await.unwrap();
    report(&control, &claimed, &fingerprint, 3, Completed, now).await;

    // Abandoned chunk ciphertext does not linger in PostgreSQL.
    sqlx::query("INSERT INTO zuse_rust_launch_assets(workspace_id,asset_id,state,created_at,updated_at) VALUES($1,'asset_abandoned_upload_fixture','uploading',$2,$2)").bind(WS).bind(now).execute(&application).await.unwrap();
    sqlx::query("INSERT INTO zuse_rust_task_asset_chunks(workspace_id,asset_id,chunk_sequence,size_bytes,sha256,ciphertext,ready,updated_at) VALUES($1,'asset_abandoned_upload_fixture',0,1,'x','sealed',false,$2)").bind(WS).bind(now).execute(&application).await.unwrap();
    control
        .sweep_asset_releases_once(now + crate::runtime::assets::STALE_CHUNK_CIPHERTEXT_MS - 1)
        .await
        .unwrap();
    let lingering = || async {
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM zuse_rust_task_asset_chunks WHERE ciphertext IS NOT NULL",
        )
        .fetch_one(&application)
        .await
        .unwrap()
    };
    assert_eq!(
        lingering().await,
        1,
        "an upload in progress keeps its chunk"
    );
    control
        .sweep_asset_releases_once(now + crate::runtime::assets::STALE_CHUNK_CIPHERTEXT_MS)
        .await
        .unwrap();
    assert_eq!(lingering().await, 0);

    // Never delivered: expires after the TTL, not a moment before.
    let ttl = crate::runtime::assets::UNDELIVERED_ASSET_TTL_MS;
    control
        .sweep_asset_releases_once(now + ttl - 1)
        .await
        .unwrap();
    assert_eq!(state(&application, &refused).await.0, "ready");
    assert!(control.sweep_asset_releases_once(now + ttl).await.unwrap());
    assert_eq!(
        state(&application, &refused).await,
        ("released".into(), Some("expired".into()))
    );
    assert_eq!(objects.keys(refused_id), 0);
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT state FROM zuse_rust_launch_assets WHERE asset_id='asset_abandoned_upload_fixture'").fetch_one(&application).await.unwrap(),
        "released"
    );

    // Workspace deletion still removes every row, holds included.
    let mut purge = application.begin().await.unwrap();
    super::super::super::deletion::purge_metadata(&mut purge, WS)
        .await
        .unwrap();
    purge.commit().await.unwrap();
    let remaining: i64 = sqlx::query_scalar("SELECT (SELECT count(*) FROM zuse_rust_launch_asset_holds)+(SELECT count(*) FROM zuse_rust_launch_assets)").fetch_one(&application).await.unwrap();
    assert_eq!(remaining, 0);

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
