//! Real PG/control path with a caller-cancelled remote object operation.
use super::*;
use crate::objects::Objects;
use sha2::Digest as _;
use std::sync::Arc;
// Deterministic remote-I/O boundary for PostgreSQL/control isolation tests.
// A submitted PUT survives cancellation of its client future, as a real R2
// request may continue committing after the caller disconnects.
use std::collections::BTreeMap;
use tokio::sync::{Mutex, Semaphore};
struct GatedObjects {
    bytes: Arc<Mutex<BTreeMap<String, Vec<u8>>>>,
    entered: Arc<Semaphore>,
    release: Arc<Semaphore>,
    finished: Arc<Semaphore>,
}
impl GatedObjects {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            bytes: Arc::new(Mutex::new(BTreeMap::new())),
            entered: Arc::new(Semaphore::new(0)),
            release: Arc::new(Semaphore::new(0)),
            finished: Arc::new(Semaphore::new(0)),
        })
    }
    async fn wait_entered(&self) {
        self.entered.acquire().await.unwrap().forget();
    }
    fn finish_remote_put(&self) {
        self.release.add_permits(1);
    }
    async fn wait_finished(&self) {
        self.finished.acquire().await.unwrap().forget();
    }
}
#[async_trait::async_trait]
impl crate::objects::Objects for GatedObjects {
    async fn get(&self, key: &str) -> Result<Option<Vec<u8>>, crate::error::ApiError> {
        Ok(self.bytes.lock().await.get(key).cloned())
    }
    async fn put(&self, key: &str, bytes: &[u8]) -> Result<(), crate::error::ApiError> {
        let key = key.to_owned();
        let value = bytes.to_vec();
        let state = self.bytes.clone();
        let entered = self.entered.clone();
        let release = self.release.clone();
        let finished = self.finished.clone();
        let remote = tokio::spawn(async move {
            entered.add_permits(1);
            release.acquire().await.unwrap().forget();
            let mut state = state.lock().await;
            if state.get(&key).is_some_and(|prior| prior != &value) {
                return Err(crate::error::ApiError::conflict(
                    "fixture_immutable_object_conflict",
                ));
            }
            state.insert(key, value);
            finished.add_permits(1);
            Ok(())
        });
        remote
            .await
            .map_err(|_| crate::error::ApiError::unavailable("fixture_remote_put_failed"))?
    }
    fn supports_purge(&self) -> bool {
        true
    }
    async fn purge_prefix(&self, prefix: &str) -> Result<bool, crate::error::ApiError> {
        self.bytes
            .lock()
            .await
            .retain(|key, _| !key.starts_with(prefix));
        Ok(true)
    }
}

#[tokio::test]
#[ignore = "requires explicit isolated localhost PostgreSQL"]
async fn v2_large_tool_put_keeps_control_and_new_admission_independent_and_reconciles_late_completion()
 {
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
    use base64::Engine as _;
    // A second fixture path exercises real preparation/encryption and a fresh
    // device's API presentation without claiming a certified managed base.
    let master = crate::crypto::MasterKey::parse(
        &base64::engine::general_purpose::URL_SAFE_NO_PAD.encode([48u8; 32]),
    )
    .unwrap();
    let signing = ed25519_dalek::SigningKey::from_bytes(&[49; 32]);
    let jwk = json!({"kty":"OKP","crv":"Ed25519","d":base64::engine::general_purpose::URL_SAFE_NO_PAD.encode([49u8;32]),"x":base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(signing.verifying_key().as_bytes())});
    let control = RuntimeControl::from_pool(
        application.clone(),
        crate::crypto::MasterKey::parse(
            &base64::engine::general_purpose::URL_SAFE_NO_PAD.encode([48u8; 32]),
        )
        .unwrap(),
        crate::tickets::TicketSigner::from_jwk(&jwk.to_string(), "https://api.example.test")
            .unwrap(),
    );
    let access = crate::access::Access {
        actor: crate::auth::Principal {
            account_id: "user-test".into(),
        },
        scope: crate::access::Scope::Personal,
        owner_id: "user-test".into(),
        membership: None,
    };
    let objects = GatedObjects::new();
    let control = Arc::new(control.with_transcript_objects(objects.clone()));
    fn plan(id: &str, command: bool, now: i64) -> Value {
        json!({
            "workspaceId":id,"ownerId":"user-test","projectId":"project","buildId":"legacy-image","provider":"boxd","chatId":format!("chat-{id}"),"sessionId":format!("session-{id}"),"branch":format!("task/{id}"),"baseRef":"main","wrappedTranscriptKey":"fixture-ciphertext","idempotencyKey":format!("key:{id}"),"requestConfig":{"runtimeGeneration":1,"gatewayEpoch":1,"commandDeliveryMode":"server-admitted-v2","rustCommandAdmissionVersion":2},"bootTokenHash":"fixture-hash","bootExpiresAt":now+60000,"commandId":format!("launch:{id}"),"turnId":format!("turn:{id}"),"launchCiphertext":"fixture-ciphertext","launchExpiresAt":now+60000,"admissionCiphertext":"fixture-ciphertext","operationId":format!("allocate:{id}"),"snapshotId":"fixture-snapshot","machineName":format!("machine-{id}"),"storageIncarnationId":format!("disk:{id}"),"sealedBootToken":"fixture-ciphertext","jobId":format!("job:{id}"),"now":now,
            "initialTaskCommand":{"policyCiphertext":"fixture-ciphertext","sessionCiphertext":"fixture-ciphertext","actorId":"user-test","command":if command {json!({"requestDigest":"fixture-request-digest","envelope":{"fingerprint":"fixture-fingerprint","ciphertext":"fixture-ciphertext"},"envelopeBytes":128,"presentationCiphertext":"fixture-ciphertext","acceptance":{"commandId":format!("launch:{id}"),"durablyAdmitted":true}})}else{Value::Null}}
        })
    }

    let prepare = |id: &str| {
        let (_, wrapped) = master.create_transcript_key("user-test", id).unwrap();
        let mut actual = plan(id, true, now);
        actual["wrappedTranscriptKey"] = json!(wrapped);
        actual["requestConfig"]["runtimeRpcProtocol"] =
            json!(zuse_protocol::cloud_gateway::RUST_RPC);
        actual["requestConfig"]["rustCommandMailboxEnabled"] = json!(true);
        actual["requestConfig"]["agent"] = json!("codex");
        actual["requestConfig"]["agentConnectionId"] = json!("connection");
        actual["requestConfig"]["agentConnectionOwnerId"] = json!("user-test");
        actual["requestConfig"]["rustRuntimeStorage"] =
            json!({"storageIncarnationId":format!("disk:{id}")});
        let row = json!({"workspace_id":id,"account_id":"user-test","chat_id":format!("chat-{id}"),"initial_session_id":format!("session-{id}"),"wrapped_transcript_key":wrapped,"state":"provisioning","desired_state":"ready","request_config":actual["requestConfig"]});
        let intent = json!({"workspaceId":id,"chatId":format!("chat-{id}"),"sessionId":format!("session-{id}"),"commandId":format!("launch:{id}"),"turnId":format!("turn:{id}"),"runtimeMode":"approval-required","model":"fixture","reasoning":null,"modelOptions":{},"firstMessage":"Initial durable task","title":"Fixture","attachments":[]});
        let release = json!({"harnesses":[{"agent":"codex","catalog":{"id":"codex","name":"Codex","defaultModel":"fixture","models":[{"id":"fixture","label":"Fixture"}]}}]});
        actual["initialTaskCommand"] = serde_json::to_value(
            control
                .prepare_initial_task_command(&row, &access, &intent, &release, now)
                .unwrap(),
        )
        .unwrap();
        actual["admissionCiphertext"] = json!(
            master
                .seal_api(
                    &format!("task-admission:user-test:{id}"),
                    &intent.to_string()
                )
                .unwrap()
        );
        actual
    };
    sqlx::query_scalar::<_, Value>(WRITE_V2.as_str())
        .bind(prepare("io-task"))
        .fetch_one(&application)
        .await
        .unwrap();
    sqlx::query("UPDATE api_cloud_workspaces SET state='ready',runtime_state='online',provider_sandbox_id='machine-'||workspace_id,runtime_credential_hash=$2,request_config=request_config||$3 WHERE workspace_id=$1").bind("io-task").bind(hash("runtime-fixture")).bind(json!({"runtimeBootstrapReceipt":{"workspaceId":"io-task","generation":1,"gatewayEpoch":1},"runtimeCredentialExpiresAtMs":now+3600000})).execute(&application).await.unwrap();
    sqlx::query("INSERT INTO zuse_rust_tool_output_refs VALUES('io-task','session-io-task/output','session-io-task')").execute(&application).await.unwrap();
    let chunk = zuse_cloud_runtime::tool_output::ToolOutputChunk {
        blob_ref: "session-io-task/output".into(),
        offset: 0,
        next_offset: 65536,
        total_bytes: 200000,
        text: "x".repeat(65536),
    };
    let upload = tokio::spawn({
        let control = control.clone();
        let chunk = chunk.clone();
        async move {
            control
                .tool_output_upload("io-task", "runtime-fixture", chunk, now)
                .await
        }
    });
    tokio::time::timeout(std::time::Duration::from_secs(2), objects.wait_entered())
        .await
        .unwrap();
    let request:zuse_protocol::cloud_task_commands::Request=serde_json::from_value(json!({"commandId":"stop:io-task","sessionId":"session-io-task","action":{"type":"submit","messageId":"stop-message","payload":{"kind":"interrupt"}}})).unwrap();
    let stop = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        control.task_command_submit("io-task", &access, request, now),
    )
    .await
    .expect("tool PUT blocked Stop")
    .unwrap();
    assert_eq!(stop["durablyAdmitted"], true);
    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        let mut tx = application.begin().await.unwrap();
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended('cloud-create:user-test',0))")
            .execute(&mut *tx)
            .await
            .unwrap();
        sqlx::query_scalar::<_, Value>(WRITE_V2.as_str())
            .bind(prepare("independent-task"))
            .fetch_one(&mut *tx)
            .await
            .unwrap();
        tx.commit().await.unwrap();
    })
    .await
    .expect("tool PUT blocked new-task admission");
    // The client goes away but its separately executing remote PUT continues.
    upload.abort();
    let _ = upload.await;
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM zuse_rust_object_write_intents WHERE workspace_id='io-task'"
        )
        .fetch_one(&application)
        .await
        .unwrap(),
        1
    );
    sqlx::query("UPDATE api_cloud_workspaces SET state='deleted',desired_state='deleted',runtime_credential_hash=NULL WHERE workspace_id='io-task'").execute(&application).await.unwrap();
    // Simulate the ordinary deletion purge and FK-backed ciphertext removal.
    objects.purge_prefix("workspaces/io-task/").await.unwrap();
    let mut purge = application.begin().await.unwrap();
    super::super::super::deletion::purge_metadata(&mut purge, "io-task")
        .await
        .unwrap();
    purge.commit().await.unwrap();
    sqlx::query("DELETE FROM api_cloud_workspaces WHERE workspace_id='io-task'")
        .execute(&application)
        .await
        .unwrap();
    objects.finish_remote_put();
    tokio::time::timeout(std::time::Duration::from_secs(1), objects.wait_finished())
        .await
        .unwrap();
    assert_eq!(
        objects.bytes.lock().await.len(),
        1,
        "fixture failed to produce a late remote object"
    );
    assert!(
        control
            .reconcile_object_write_once(now + 120000)
            .await
            .unwrap()
    );
    assert!(
        objects.bytes.lock().await.is_empty(),
        "late object survived durable cleanup reconciliation"
    );
    assert_eq!(sqlx::query_scalar::<_,i64>("SELECT count(*) FROM zuse_rust_object_write_intents WHERE workspace_id='io-task' AND cleanup_attempts=1").fetch_one(&application).await.unwrap(),1,"an empty purge must not erase an unknown physical attempt");
    // Attachments and their manifest are available while the computer sleeps.
    // Both physical PUTs leave the control lane free and publish only after ACK.
    sqlx::query("UPDATE api_cloud_workspaces SET state='paused',desired_state='paused',runtime_state='offline' WHERE workspace_id='independent-task'").execute(&application).await.unwrap();
    let make_access = || crate::access::Access {
        actor: crate::auth::Principal {
            account_id: "user-test".into(),
        },
        scope: crate::access::Scope::Personal,
        owner_id: "user-test".into(),
        membership: None,
    };
    let bytes = b"private task attachment bytes";
    let asset_chunk = tokio::spawn({
        let control = control.clone();
        let access = make_access();
        let bytes = bytes.to_vec();
        async move {
            control
                .workspace_asset_chunk(
                    "independent-task",
                    &access,
                    zuse_protocol::cloud_assets::Chunk {
                        idempotency_key: "workspace:independent-task".into(),
                        upload_id: "attachment".into(),
                        seq: 0,
                        bytes: base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes),
                    },
                    now,
                )
                .await
        }
    });
    tokio::time::timeout(std::time::Duration::from_secs(2), objects.wait_entered())
        .await
        .unwrap();
    for (cid, phase) in [("asset-stop-chunk", "chunk")] {
        let command=serde_json::from_value(json!({"commandId":cid,"sessionId":"session-independent-task","action":{"type":"submit","messageId":cid,"payload":{"kind":"interrupt"}}})).unwrap();
        let accepted = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            control.task_command_submit("independent-task", &access, command, now),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(
            accepted["durablyAdmitted"], true,
            "asset {phase} blocked Stop"
        );
    }
    objects.finish_remote_put();
    objects.wait_finished().await;
    asset_chunk.await.unwrap().unwrap();
    let sha = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(sha2::Sha256::digest(bytes));
    let manifest = tokio::spawn({
        let control = control.clone();
        let access = make_access();
        let sha = sha.clone();
        async move {
            control
                .workspace_asset_commit(
                    "independent-task",
                    &access,
                    zuse_protocol::cloud_assets::Commit {
                        idempotency_key: "workspace:independent-task".into(),
                        upload_id: "attachment".into(),
                        mime_type: "text/plain".into(),
                        original_name: "attachment.txt".into(),
                        size_bytes: bytes.len(),
                        sha256: sha,
                    },
                    now,
                )
                .await
        }
    });
    tokio::time::timeout(std::time::Duration::from_secs(2), objects.wait_entered())
        .await
        .unwrap();
    let command=serde_json::from_value(json!({"commandId":"asset-stop-manifest","sessionId":"session-independent-task","action":{"type":"submit","messageId":"asset-stop-manifest","payload":{"kind":"interrupt"}}})).unwrap();
    let accepted = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        control.task_command_submit("independent-task", &access, command, now),
    )
    .await
    .expect("manifest PUT blocked Stop")
    .unwrap();
    assert_eq!(accepted["durablyAdmitted"], true);
    objects.finish_remote_put();
    objects.wait_finished().await;
    let committed = manifest.await.unwrap().unwrap();
    let asset = committed["asset"]["assetId"].as_str().unwrap();
    let page = control
        .workspace_asset_read("independent-task", &access, asset, 0)
        .await
        .unwrap();
    assert_eq!(
        base64::engine::general_purpose::STANDARD
            .decode(page["data"].as_str().unwrap())
            .unwrap(),
        bytes
    );
    assert_eq!(
        sqlx::query_scalar::<_, String>(
            "SELECT state FROM api_cloud_workspaces WHERE workspace_id='independent-task'"
        )
        .fetch_one(&application)
        .await
        .unwrap(),
        "paused"
    );
    assert_eq!(sqlx::query_scalar::<_,i64>("SELECT count(*) FROM zuse_rust_object_write_intents WHERE workspace_id='independent-task'").fetch_one(&application).await.unwrap(),0);
    // Old runtime output may physically arrive after replacement, but cannot
    // publish its readiness under a new runtime generation/disk/source fence.
    for (index,change) in [
        json!({"runtimeGeneration":2,"runtimeBootstrapReceipt":{"workspaceId":"FILL","generation":2,"gatewayEpoch":1}}),
        json!({"rustRuntimeStorage":{"storageIncarnationId":"replacement-disk"}}),
        json!({"destructionFence":2}),
        json!({"_source":"replacement-machine"}),
    ].into_iter().enumerate(){
        let id=format!("fenced-{index}");
        sqlx::query_scalar::<_,Value>(WRITE_V2.as_str()).bind(prepare(&id)).fetch_one(&application).await.unwrap();
        sqlx::query("UPDATE api_cloud_workspaces SET state='ready',runtime_state='online',provider_sandbox_id='machine-'||workspace_id,runtime_credential_hash=$2,request_config=request_config||$3 WHERE workspace_id=$1").bind(&id).bind(hash("runtime-fixture")).bind(json!({"runtimeBootstrapReceipt":{"workspaceId":id,"generation":1,"gatewayEpoch":1},"runtimeCredentialExpiresAtMs":now+3600000})).execute(&application).await.unwrap();
        let reference=format!("session-{id}/output");
        sqlx::query("INSERT INTO zuse_rust_tool_output_refs VALUES($1,$2,$3)").bind(&id).bind(&reference).bind(format!("session-{id}")).execute(&application).await.unwrap();
        let upload=tokio::spawn({let control=control.clone();let id=id.clone();let mut chunk=chunk.clone();chunk.blob_ref=reference;async move{control.tool_output_upload(&id,"runtime-fixture",chunk,now).await}});
        tokio::time::timeout(std::time::Duration::from_secs(2),objects.wait_entered()).await.unwrap();
        let mut change=change;
        if change["runtimeBootstrapReceipt"].is_object(){change["runtimeBootstrapReceipt"]["workspaceId"]=json!(id);}
        if index==0 {sqlx::query("UPDATE zuse_rust_control_owners SET generation=2 WHERE resource_kind='workspace' AND resource_id=$1").bind(&id).execute(&application).await.unwrap();}
        if let Some(source)=change.as_object_mut().and_then(|object|object.remove("_source")) {
            sqlx::query("UPDATE api_cloud_workspaces SET provider_sandbox_id=$2 WHERE workspace_id=$1").bind(&id).bind(format!("{}-{id}",source.as_str().unwrap())).execute(&application).await.unwrap();
        }
        sqlx::query("UPDATE api_cloud_workspaces SET request_config=request_config||$2 WHERE workspace_id=$1").bind(&id).bind(change).execute(&application).await.unwrap();
        objects.finish_remote_put();objects.wait_finished().await;
        assert!(upload.await.unwrap().is_err(),"old runtime published output after its binding changed");
        assert!(!sqlx::query_scalar::<_,bool>("SELECT ready FROM zuse_rust_tool_output_chunks WHERE workspace_id=$1").bind(&id).fetch_one(&application).await.unwrap());
        sqlx::query("UPDATE api_cloud_workspaces SET state='deleted',desired_state='deleted' WHERE workspace_id=$1").bind(&id).execute(&application).await.unwrap();
    }
    // A delivery ACK does not release the next model turn. Execution reports
    // are generation-bound at lease and only actual terminal progress releases it.
    sqlx::query_scalar::<_, Value>(WRITE_V2.as_str())
        .bind(prepare("execution-task"))
        .fetch_one(&application)
        .await
        .unwrap();
    let enrollment = json!({"runtimeBootstrapReceipt":{"workspaceId":"execution-task","generation":1,"gatewayEpoch":1},"runtimeCredentialExpiresAtMs":now+3600000,"runtimeCredentialKeyThumbprint":"key","runtimeSigningKeyThumbprint":"sign","rustStorageEnrollment":{"phase":"committed","generation":1,"sourceSandboxId":"execution-machine","storageIncarnationId":"disk:execution-task","credentialKeyThumbprint":"key","signingKeyThumbprint":"sign","capabilities":["rust-session-commands-v1",zuse_protocol::cloud_task_commands::CAPABILITY]}});
    sqlx::query("UPDATE api_cloud_workspaces SET state='ready',desired_state='ready',runtime_state='online',provider_sandbox_id='execution-machine',runtime_credential_hash=$2,request_config=request_config||$3 WHERE workspace_id=$1").bind("execution-task").bind(hash("runtime-fixture")).bind(enrollment).execute(&application).await.unwrap();
    let followup=serde_json::from_value(json!({"commandId":"run:second","sessionId":"session-execution-task","action":{"type":"submit","messageId":"turn:second","payload":{"kind":"run","messageId":"turn:second","request":{"prompt":"second explicit user turn","harness":"codex","model":"fixture","cwd":".","sandbox":"workspace-write"}}}})).unwrap();
    control
        .task_command_submit("execution-task", &access, followup, now)
        .await
        .unwrap();
    let lease_request = zuse_protocol::cloud_commands::LeaseRequest {
        control_only: false,
        max_items: None,
        storage_incarnation_id: "disk:execution-task".into(),
        wait_ms: Some(0),
        after_revision: None,
    };
    let first = control
        .command_lease("execution-task", "runtime-fixture", &lease_request, now)
        .await
        .unwrap();
    assert_eq!(first.leases.len(), 1);
    assert_eq!(first.leases[0].command.command_id, "launch:execution-task");
    let execution:(i64,String)=sqlx::query_as("SELECT runtime_generation,storage_incarnation_id FROM zuse_rust_task_command_execution WHERE workspace_id='execution-task' AND command_id='launch:execution-task'").fetch_one(&application).await.unwrap();
    assert_eq!(execution, (1, "disk:execution-task".into()));
    // Legacy encrypted command submission cannot create a second authority lane.
    assert!(
        matches!(control.command_enqueue("execution-task",&access,first.leases[0].command.clone(),now).await,Err(error) if error.code=="server_command_admission_required")
    );
    control
        .command_ack(
            "execution-task",
            "runtime-fixture",
            &zuse_protocol::cloud_commands::RuntimeAcknowledgment {
                command_id: first.leases[0].command.command_id.clone(),
                lease_token: first.leases[0].lease_token.clone(),
                fingerprint: first.leases[0].command.fingerprint.clone(),
                state: zuse_protocol::cloud_commands::AcknowledgmentState::Applied,
                category: None,
                result_iv: None,
                result_ciphertext: None,
            },
            now,
        )
        .await
        .unwrap();
    assert!(
        control
            .command_lease("execution-task", "runtime-fixture", &lease_request, now)
            .await
            .unwrap()
            .leases
            .is_empty(),
        "a delivery ACK released a still-pending model turn"
    );
    let fingerprint = first.leases[0].command.fingerprint.clone();
    for (revision, state) in [
        (
            1,
            zuse_protocol::cloud_task_commands::ExecutionState::Starting,
        ),
        (
            2,
            zuse_protocol::cloud_task_commands::ExecutionState::Completed,
        ),
    ] {
        control
            .task_command_progress(
                "execution-task",
                "runtime-fixture",
                &zuse_protocol::cloud_task_commands::Progress {
                    command_id: "launch:execution-task".into(),
                    fingerprint: fingerprint.clone(),
                    storage_incarnation_id: "disk:execution-task".into(),
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
    let next = control
        .command_lease("execution-task", "runtime-fixture", &lease_request, now)
        .await
        .unwrap();
    assert_eq!(next.leases.len(), 1);
    assert_eq!(next.leases[0].command.command_id, "run:second");
    // After a real generation replacement, an unresolved admitted execution is
    // uncertain, never replayed or inferred complete from its delivery receipt.
    sqlx::query("UPDATE zuse_rust_control_owners SET generation=2 WHERE resource_kind='workspace' AND resource_id='execution-task'").execute(&application).await.unwrap();
    sqlx::query("UPDATE api_cloud_workspaces SET request_config=request_config||$1 WHERE workspace_id='execution-task'").bind(json!({"runtimeGeneration":2,"runtimeBootstrapReceipt":{"workspaceId":"execution-task","generation":2,"gatewayEpoch":1},"rustStorageEnrollment":{"phase":"committed","generation":2,"sourceSandboxId":"execution-machine","storageIncarnationId":"disk:execution-task","credentialKeyThumbprint":"key","signingKeyThumbprint":"sign","capabilities":["rust-session-commands-v1",zuse_protocol::cloud_task_commands::CAPABILITY]}})).execute(&application).await.unwrap();
    assert!(
        control
            .command_lease("execution-task", "runtime-fixture", &lease_request, now)
            .await
            .unwrap()
            .leases
            .is_empty()
    );
    assert_eq!(sqlx::query_scalar::<_,String>("SELECT state FROM zuse_rust_task_command_execution WHERE workspace_id='execution-task' AND command_id='run:second'").fetch_one(&application).await.unwrap(),"outcome-unknown");
    super::interrupts::exercise(&control, &application, &access, now, &prepare).await;
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
