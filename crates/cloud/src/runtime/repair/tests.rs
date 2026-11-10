use super::*;
#[test]
fn runtime_repair_receipts_are_identity_bound_bounded_and_strip_private_fields() {
    let op = json!({"operation_id":"repair","workspace_id":"task","generation":7,"storage_incarnation_id":"disk","command_ids":["attempted"]});
    let receipt = json!({"version":1,"operationId":"repair","workspaceId":"task","generation":7,"storageIncarnationId":"disk","phase":"ready","graceful":false,"childPid":123,"privateOutput":"must-not-persist","recovery":{"interruptedChats":["chat"],"quarantinedCommandCount":1,"receipts":[{"commandId":"attempted","fingerprint":"bound","state":"applied","output":"private"}]}});
    let safe = worker::summary(&receipt, &op).unwrap();
    assert!(!safe.to_string().contains("private"));
    assert_eq!(safe["childPid"], 123);
    assert_eq!(safe["recovery"]["receipts"][0]["state"], "applied");
    for field in ["operationId", "workspaceId", "storageIncarnationId"] {
        let mut bad = receipt.clone();
        bad[field] = json!("foreign");
        assert!(worker::summary(&bad, &op).is_err());
    }
    let mut bad = receipt.clone();
    bad["generation"] = json!(8);
    assert!(worker::summary(&bad, &op).is_err());
    let mut bad = receipt.clone();
    bad["recovery"]["receipts"][0]["commandId"] = json!("unattempted");
    assert!(worker::summary(&bad, &op).is_err());
    let mut bad = receipt.clone();
    bad["recovery"]["receipts"]
        .as_array_mut()
        .unwrap()
        .push(receipt["recovery"]["receipts"][0].clone());
    assert!(worker::summary(&bad, &op).is_err());
    let mut bad = receipt.clone();
    bad["recovery"]["receipts"][0]["state"] = json!("running");
    assert!(worker::summary(&bad, &op).is_err());
    let mut bad = receipt.clone();
    bad["recovery"]["interruptedChats"] = json!(["bad\nchat"]);
    assert!(worker::summary(&bad, &op).is_err());
}
#[test]
fn runtime_repair_attention_holds_new_leases_until_explicit_resolution() {
    for phase in [
        "quiescing",
        "stopping",
        "starting",
        "waiting-for-runtime",
        "attention-required",
        "failed",
    ] {
        assert!(leases_blocked(
            &json!({"request_config":{"rustRuntimeRepair":{"phase":phase}}})
        ));
    }
    assert!(!leases_blocked(
        &json!({"request_config":{"rustRuntimeRepair":{"phase":"ready"}}})
    ));
    assert!(!leases_blocked(&json!({"request_config":{}})));
}

use crate::{access::Scope, auth::Principal, crypto::MasterKey, tickets::TicketSigner};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use std::sync::{
    Mutex,
    atomic::{AtomicBool, Ordering},
};
struct SupervisorFixture {
    pool: PgPool,
    actions: Mutex<Vec<String>>,
    lost_start: AtomicBool,
    bridge: Option<tokio::sync::Mutex<RealSupervisorBridge>>,
}
#[async_trait]
impl RepairProvider for SupervisorFixture {
    fn provider_id(&self) -> &'static str {
        "boxd"
    }
    async fn inspect(&self, id: &str) -> Result<Option<Machine>, ProviderError> {
        assert_eq!(id, "machine-bob");
        tokio::time::timeout(
            std::time::Duration::from_secs(1),
            sqlx::query(
                "UPDATE api_cloud_workspaces SET revision=revision+1 WHERE workspace_id='ws-bob'",
            )
            .execute(&self.pool),
        )
        .await
        .expect("no workspace SQL lock across provider effects")
        .unwrap();
        Ok(Some(Machine {
            id: id.into(),
            name: "retained".into(),
            state: MachineState::Running,
            vcpu: None,
            memory_bytes: None,
        }))
    }
    async fn rpc(&self, id: &str, request: &Value) -> Result<Value, ProviderError> {
        assert_eq!(id, "machine-bob");
        let action = request["action"].as_str().unwrap();
        self.actions.lock().unwrap().push(action.into());
        let reply = if let Some(bridge) = &self.bridge {
            bridge.lock().await.request(request).await?
        } else {
            let mut reply = request.clone();
            reply["phase"] = json!(if action == "repair-start" {
                "ready"
            } else {
                "stopped"
            });
            reply["graceful"] = json!(false);
            if action == "repair-start" {
                reply["recovery"] = json!({"interruptedChats":["old-chat"],"quarantinedCommandCount":1,"receipts":[{"commandId":"attempted","fingerprint":null,"state":null}]});
            }
            reply
        };
        if action == "repair-start" && self.lost_start.swap(false, Ordering::SeqCst) {
            return Err(ProviderError::OutcomeUnknown);
        }
        Ok(reply)
    }
}
#[tokio::test]
#[ignore = "requires explicit isolated localhost PostgreSQL"]
async fn runtime_repair_retains_machine_quarantines_attempts_and_waits_for_runtime_ack() {
    run_repair_fixture(false, "repository").await;
}
#[tokio::test]
#[ignore = "requires isolated localhost PostgreSQL and Linux subprocesses"]
async fn runtime_repair_real_supervisor_preserves_committed_action_and_retires_detached_orphans() {
    run_repair_fixture(true, "repository").await;
}
static REPAIR_FIXTURE_SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
#[tokio::test]
#[ignore = "requires explicit isolated localhost PostgreSQL"]
async fn runtime_repair_retries_failed_credentials_without_replacing_initial_message() {
    run_repair_fixture(false, "credentials").await;
}
async fn run_repair_fixture(real: bool, stage: &str) {
    let failure_code = match stage {
        "repository" => "repository_preparation_failed",
        "credentials" => "credential_preparation_failed",
        _ => panic!("unsupported fixture preparation stage"),
    };
    let _serial = REPAIR_FIXTURE_SERIAL.lock().await;
    let url = std::env::var("ZUSE_CLOUD_TEST_DATABASE_URL").expect("isolated PostgreSQL required");
    assert!(matches!(
        reqwest::Url::parse(&url).unwrap().host_str(),
        Some("localhost" | "127.0.0.1" | "::1")
    ));
    let admin = PgPool::connect(&url).await.unwrap();
    let schema = format!("repair_{}", uuid::Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&admin)
        .await
        .unwrap();
    let scope = schema.clone();
    let pool = sqlx::postgres::PgPoolOptions::new()
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
        include_str!("../../../tests/fixtures/billing.sql"),
        jobs::MIGRATION,
        super::super::mailbox::MIGRATION,
        super::super::lifecycle::MIGRATION,
        super::super::allocation::MIGRATION,
        include_str!("../../../migrations/0012_provider_receipts.sql"),
        "CREATE TABLE cloud_managed_bases(provider text,release_id text,artifact_id text,architecture text,capabilities jsonb,state text,created_at bigint,runtime_manifest jsonb,PRIMARY KEY(provider,release_id))",
        crate::managed_bases::candidate::MIGRATION,
        include_str!("../../../migrations/0023_managed_base_admission_lock.sql"),
        "CREATE TABLE zuse_rust_account_event_outbox(owner_id text,kind text,resource_id text,workspace_id text,payload jsonb,created_at bigint)",
        include_str!("../../../migrations/0021_runtime_repairs.sql"),
    ] {
        sqlx::raw_sql(sql).execute(&pool).await.unwrap();
    }
    let now = chrono::Utc::now().timestamp_millis();
    let candidate_pin =
        crate::managed_bases::candidate::fixture_registered_pin(&pool, "user-bob", now)
            .await
            .unwrap();
    sqlx::query("INSERT INTO api_cloud_billing_periods VALUES('repair-period','user-bob','polar',NULL,'manual','USD',$1,$2,1000,1000,500,500,$1)").bind(now-1000).bind(now+86400000).execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO zuse_rust_control_owners VALUES('workspace','ws-bob','rust',1,$1)")
        .bind(now)
        .execute(&pool)
        .await
        .unwrap();
    let config = json!({"runtimeGeneration":1,"gatewayEpoch":1,"runtimeRpcProtocol":zuse_protocol::cloud_gateway::RUST_RPC,"runtimeBootstrapReceipt":{"workspaceId":"ws-bob","generation":1,"gatewayEpoch":1},"runtimeCredentialExpiresAtMs":now+3600000,"rustCommandMailboxEnabled":true,"rustRuntimeRepairVersion":1,"rustRuntimeSupervisorVersion":1,"rustMemorySleepVersion":1,"runtimeCredentialKeyThumbprint":"key","runtimeSigningKeyThumbprint":"signing","rustRuntimeStorage":{"storageIncarnationId":"disk"},"rustStorageEnrollment":{"phase":"committed","generation":1,"sourceSandboxId":"machine-bob","storageIncarnationId":"disk","credentialKeyThumbprint":"key","signingKeyThumbprint":"signing"}});
    sqlx::query("UPDATE api_cloud_workspaces SET request_config=$1,runtime_credential_hash=$2 WHERE workspace_id='ws-bob'")
        .bind(config)
        .bind(hash("runtime-fixture"))
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO zuse_rust_mailboxes VALUES('ws-bob',2,2)")
        .execute(&pool)
        .await
        .unwrap();
    for (cid, seq, leased, state) in [
        ("attempted", 1, true, "leased"),
        ("pending", 2, false, "accepted"),
    ] {
        sqlx::query("INSERT INTO zuse_rust_mailbox_commands(workspace_id,command_id,workspace_sequence,revision,lane,fingerprint,envelope,envelope_bytes,state,ever_leased,accepted_at,updated_at) VALUES('ws-bob',$1,$2,$2,'session','fingerprint','{}',1,$3,$4,$5,$5)").bind(cid).bind(seq as i64).bind(state).bind(leased).bind(now).execute(&pool).await.unwrap();
    }
    let signing = ed25519_dalek::SigningKey::from_bytes(&[48; 32]);
    let jwk = json!({"kty":"OKP","crv":"Ed25519","d":URL_SAFE_NO_PAD.encode([48;32]),"x":URL_SAFE_NO_PAD.encode(signing.verifying_key().as_bytes())});
    let control = RuntimeControl::from_pool(
        pool.clone(),
        MasterKey::parse(&URL_SAFE_NO_PAD.encode([49; 32])).unwrap(),
        TicketSigner::from_jwk(&jwk.to_string(), "https://api.example.test").unwrap(),
    )
    .with_staging_base_pin(Some(candidate_pin.clone()));
    // A failed retained transport remains a typed attention condition on the
    // ORIGINAL lifecycle operation. Its provider phase is not forged Ready.
    let retained_operation = uuid::Uuid::new_v4().to_string();
    sqlx::query("INSERT INTO zuse_rust_lifecycle_operations(operation_id,workspace_id,command_id,generation,source_machine_id,storage_incarnation_id,action,phase,created_at,updated_at) VALUES($1,'ws-bob','memory-resume',1,'machine-bob','disk','resume','waking',$2,$2)")
        .bind(&retained_operation).bind(now).execute(&pool).await.unwrap();
    sqlx::query("UPDATE api_cloud_workspaces SET state='provisioning',desired_state='ready',request_config=request_config||$1 WHERE workspace_id='ws-bob'")
        .bind(json!({"agent":"codex","managedBaseReadiness":"uncertified-staging","managedReleaseId":candidate_pin.release_id,"managedCandidateSha256":candidate_pin.candidate_sha256,"managedCandidateArtifactId":candidate_pin.artifact_id,"rustLifecycleOperation":{"id":retained_operation,"action":"resume","phase":"waking"},"rustMemoryParked":{"version":1,"operationId":"previous-bound-pause","generation":1,"sourceMachineId":"machine-bob","storageIncarnationId":"disk"}})).execute(&pool).await.unwrap();
    let mut tx = pool.begin().await.unwrap();
    jobs::enqueue(
        &mut tx,
        "memory-attention-job",
        lifecycle::JOB_KIND,
        "ws-bob",
        1,
        &json!({"operationId":retained_operation}),
        now,
    )
    .await
    .unwrap();
    let (retained_row, _, _) = locked_workspace(&mut tx, "ws-bob").await.unwrap();
    let retained_op: Value = sqlx::query_scalar(
        "SELECT to_jsonb(o) FROM zuse_rust_lifecycle_operations o WHERE operation_id=$1",
    )
    .bind(&retained_operation)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    let retained_directive = zuse_protocol::cloud_lifecycle::Directive {
        full_shutdown: false,
        operation_id: retained_operation.clone(),
        action: zuse_protocol::cloud_lifecycle::Action::Resume,
        phase: "waking".into(),
        generation: 1,
        source_machine_id: "machine-bob".into(),
        storage_incarnation_id: "disk".into(),
    };
    for (operation, code) in [
        (
            "wrong-operation",
            "codex_memory_transport_recovery_required",
        ),
        (retained_operation.as_str(), "changed-error-code"),
    ] {
        assert!(
            control
                .memory_recovery_report_tx(
                    &mut tx,
                    &retained_row,
                    &retained_op,
                    &retained_directive,
                    operation,
                    code,
                    now
                )
                .await
                .is_err()
        );
    }
    tx.commit().await.unwrap();
    let stale_job = jobs::claim(&pool, lifecycle::JOB_KIND, "workspace", now, 120_000)
        .await
        .unwrap()
        .unwrap();
    assert_real_authority_withdrawal(&control, now).await;
    // Authenticated attention is permitted after the operator withdraws pilot
    // enablement: no current enable flag is needed to report a safe failure.
    let receipt = control
        .lifecycle_runtime(
            "ws-bob",
            "runtime-fixture",
            zuse_protocol::cloud_lifecycle::RuntimeRequest::RecoveryRequired {
                operation_id: retained_operation.clone(),
                code: "codex_memory_transport_recovery_required".into(),
            },
            now,
        )
        .await
        .unwrap();
    assert!(receipt["directive"].is_null());
    assert!(
        !jobs::retry(&pool, &stale_job, now + 1, "cloud_lifecycle_fenced")
            .await
            .unwrap(),
        "an in-flight worker cannot revive the failed attention operation"
    );
    assert_eq!(
        sqlx::query_scalar::<_, String>(
            "SELECT phase FROM zuse_rust_lifecycle_operations WHERE operation_id=$1"
        )
        .bind(&retained_operation)
        .fetch_one(&pool)
        .await
        .unwrap(),
        "waking"
    );
    assert_eq!(
        sqlx::query_scalar::<_, String>(
            "SELECT state FROM zuse_rust_jobs WHERE idempotency_key='memory-attention-job'"
        )
        .fetch_one(&pool)
        .await
        .unwrap(),
        "failed"
    );
    let query = control
        .lifecycle_runtime(
            "ws-bob",
            "runtime-fixture",
            zuse_protocol::cloud_lifecycle::RuntimeRequest::Query,
            now + 1,
        )
        .await
        .unwrap();
    assert!(query["directive"].is_null());
    assert_eq!(query["recoveryRequired"]["operationId"], retained_operation);
    assert!(
        control
            .lifecycle_runtime(
                "ws-bob",
                "wrong-runtime",
                zuse_protocol::cloud_lifecycle::RuntimeRequest::Query,
                now + 1
            )
            .await
            .is_err()
    );
    assert!(
        control
            .lifecycle_runtime(
                "ws-bob",
                "runtime-fixture",
                zuse_protocol::cloud_lifecycle::RuntimeRequest::Resumed {
                    operation_id: retained_operation.clone()
                },
                now + 1
            )
            .await
            .is_err(),
        "late Ready must not clear attention"
    );
    for (field, wrong, restore) in [("runtimeGeneration", json!(2), json!(1))] {
        sqlx::query("UPDATE api_cloud_workspaces SET request_config=jsonb_set(request_config,ARRAY[$1],$2) WHERE workspace_id='ws-bob'").bind(field).bind(wrong).execute(&pool).await.unwrap();
        assert!(
            control
                .lifecycle_runtime(
                    "ws-bob",
                    "runtime-fixture",
                    zuse_protocol::cloud_lifecycle::RuntimeRequest::Query,
                    now + 1
                )
                .await
                .is_err()
        );
        sqlx::query("UPDATE api_cloud_workspaces SET request_config=jsonb_set(request_config,ARRAY[$1],$2) WHERE workspace_id='ws-bob'").bind(field).bind(restore).execute(&pool).await.unwrap();
    }
    sqlx::query("UPDATE api_cloud_workspaces SET provider_sandbox_id='different-machine' WHERE workspace_id='ws-bob'").execute(&pool).await.unwrap();
    assert!(
        control
            .lifecycle_runtime(
                "ws-bob",
                "runtime-fixture",
                zuse_protocol::cloud_lifecycle::RuntimeRequest::Query,
                now + 1
            )
            .await
            .is_err()
    );
    sqlx::query("UPDATE api_cloud_workspaces SET provider_sandbox_id='machine-bob' WHERE workspace_id='ws-bob'").execute(&pool).await.unwrap();
    let attention_row: Value = sqlx::query_scalar(
        "SELECT to_jsonb(w) FROM api_cloud_workspaces w WHERE workspace_id='ws-bob'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        workspace_metadata(&attention_row)["runtimeRepairAvailable"],
        true,
        "same-machine attention can use the explicit existing runtime repair"
    );
    let bob = Access {
        actor: Principal {
            account_id: "user-bob".into(),
        },
        scope: Scope::Personal,
        owner_id: "user-bob".into(),
        membership: None,
    };
    let foreign = Access {
        actor: Principal {
            account_id: "user-alice".into(),
        },
        scope: Scope::Personal,
        owner_id: "user-alice".into(),
        membership: None,
    };
    assert!(
        control
            .restart_runtime(
                "ws-bob",
                &foreign,
                Start {
                    idempotency_key: "foreign".into()
                },
                now
            )
            .await
            .is_err()
    );
    if !real {
        let allocation = uuid::Uuid::new_v4().to_string();
        let ingress = json!({"version":1,"operationId":allocation,"generation":1,"sourceMachineId":"machine-bob","storageIncarnationId":"disk","connectedAt":now});
        let mut stages = json!({});
        stages[stage] = json!({"operationId":allocation,"state":"failed","errorCode":failure_code,"startedAt":now,"updatedAt":now+1});
        sqlx::query("UPDATE api_cloud_workspaces SET state='setup',desired_state='ready',runtime_state='connecting',status_code=$2,request_config=request_config||$1 WHERE workspace_id='ws-bob'")
            .bind(json!({"rustPreparationVersion":1,"rustAllocationOperationId":allocation,"rustPreparationIngress":ingress,"preparation":stages})).bind(failure_code).execute(&pool).await.unwrap();
        let original: Value = sqlx::query_scalar(
            "SELECT to_jsonb(w) FROM api_cloud_workspaces w WHERE workspace_id='ws-bob'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            workspace_metadata(&original)["runtimeRepairAvailable"],
            true,
            "failed ingress Core can repair without fabricated Ready"
        );
        for (field, value) in [
            ("operationId", json!(uuid::Uuid::new_v4().to_string())),
            ("generation", json!(2)),
            ("sourceMachineId", json!("different")),
            ("storageIncarnationId", json!("different")),
            ("connectedAt", json!(now + 100)),
        ] {
            let mut changed = original["request_config"].clone();
            changed["rustPreparationIngress"][field] = value;
            sqlx::query(
                "UPDATE api_cloud_workspaces SET request_config=$1 WHERE workspace_id='ws-bob'",
            )
            .bind(&changed)
            .execute(&pool)
            .await
            .unwrap();
            assert!(
                !preparation::failed_preparation_ingress(
                    &json!({"state":"setup","desired_state":"ready","runtime_state":"connecting","status_code":failure_code,"provider_sandbox_id":"machine-bob","request_config":changed})
                ),
                "mismatched ingress {field}"
            );
            assert_eq!(
                control
                    .restart_runtime(
                        "ws-bob",
                        &bob,
                        Start {
                            idempotency_key: format!("bad-ingress-{field}")
                        },
                        now
                    )
                    .await
                    .unwrap_err()
                    .code,
                "runtime_repair_requires_running_computer"
            );
        }
        sqlx::query(
            "UPDATE api_cloud_workspaces SET request_config=$1 WHERE workspace_id='ws-bob'",
        )
        .bind(&original["request_config"])
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO api_cloud_workspace_launch_intents(workspace_id,account_id,chat_id,session_id,turn_id,command_id,ciphertext,expires_at,created_at) VALUES('ws-bob','user-bob','chat','session','original-turn','original-first-command','opaque-original-intent',$1,$2)").bind(now+3600000).bind(now).execute(&pool).await.unwrap();
    }
    let op = control
        .restart_runtime(
            "ws-bob",
            &bob,
            Start {
                idempotency_key: "repair-once".into(),
            },
            now,
        )
        .await
        .unwrap();
    let operation = op["operationId"].as_str().unwrap();
    let again = control
        .restart_runtime(
            "ws-bob",
            &bob,
            Start {
                idempotency_key: "repair-once".into(),
            },
            now,
        )
        .await
        .unwrap();
    assert_eq!(again["operationId"], operation);
    if !real {
        let intent:(String,String,String)=sqlx::query_as("SELECT command_id,turn_id,ciphertext FROM api_cloud_workspace_launch_intents WHERE workspace_id='ws-bob'").fetch_one(&pool).await.unwrap();
        assert_eq!(
            intent,
            (
                "original-first-command".into(),
                "original-turn".into(),
                "opaque-original-intent".into()
            )
        );
    }

    if !real {
        let allocation:String=sqlx::query_scalar("SELECT request_config->>'rustAllocationOperationId' FROM api_cloud_workspaces WHERE workspace_id='ws-bob'").fetch_one(&pool).await.unwrap();
        sqlx::query("INSERT INTO zuse_rust_allocations(operation_id,workspace_id,generation,snapshot_id,machine_name,machine_id,storage_incarnation_id,sealed_boot_token,boot_expires_at,phase,created_at,updated_at) VALUES($1,'ws-bob',1,'clean-base','original-allocation','machine-bob','disk','expired-original-boot',$2,'launching',$2,$2)").bind(&allocation).bind(now-1).execute(&pool).await.unwrap();
        let mut tx = pool.begin().await.unwrap();
        jobs::enqueue(
            &mut tx,
            "original-allocation-held",
            super::super::allocation::JOB_KIND,
            "ws-bob",
            1,
            &json!({"operationId":allocation}),
            now,
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
        sqlx::query("UPDATE zuse_rust_jobs SET max_attempts=2 WHERE kind=$1")
            .bind(super::super::allocation::JOB_KIND)
            .execute(&pool)
            .await
            .unwrap();
    }
    let provider = SupervisorFixture {
        pool: pool.clone(),
        actions: Mutex::new(vec![]),
        lost_start: AtomicBool::new(true),
        bridge: if real {
            Some(tokio::sync::Mutex::new(RealSupervisorBridge::start().await))
        } else {
            None
        },
    };
    for step in 0..6 {
        if !real {
            sqlx::query("UPDATE zuse_rust_jobs SET due_at=0 WHERE kind=$1")
                .bind(super::super::allocation::JOB_KIND)
                .execute(&pool)
                .await
                .unwrap();
            assert!(
                control
                    .allocate_once(&NoAllocationRpc, now + 6000 + step * 100)
                    .await
                    .unwrap(),
                "active repair holds original allocation without any provider RPC"
            );
        }
        sqlx::query("UPDATE zuse_rust_jobs SET due_at=0 WHERE kind=$1")
            .bind(JOB_KIND)
            .execute(&pool)
            .await
            .unwrap();
        let result = control
            .runtime_repair_once(&provider, now + 6000 + step * 100)
            .await;
        assert_eq!(result.is_err(), step == 3, "step {step}: {result:?}");
    }
    let waiting = control
        .runtime_repair_status("ws-bob", operation, &bob)
        .await
        .unwrap();
    assert_eq!(waiting["state"], "waiting-for-runtime");
    assert_eq!(
        sqlx::query_scalar::<_, String>(
            "SELECT state FROM zuse_rust_mailbox_commands WHERE command_id='attempted'"
        )
        .fetch_one(&pool)
        .await
        .unwrap(),
        "outcome-unknown"
    );
    // Online state/config alone cannot certify the repair before its real ACK.
    sqlx::query("UPDATE api_cloud_workspaces SET state='ready',runtime_state='online',request_config=jsonb_set(request_config,'{rustLifecycleOperation,phase}','\"ready\"') WHERE workspace_id='ws-bob'").execute(&pool).await.unwrap();
    sqlx::query("UPDATE zuse_rust_jobs SET due_at=0 WHERE kind=$1")
        .bind(JOB_KIND)
        .execute(&pool)
        .await
        .unwrap();
    control
        .runtime_repair_once(&provider, now + 7700)
        .await
        .unwrap();
    assert_eq!(
        control
            .runtime_repair_status("ws-bob", operation, &bob)
            .await
            .unwrap()["state"],
        "waiting-for-runtime"
    );
    sqlx::query("UPDATE zuse_rust_lifecycle_operations SET phase='ready' WHERE operation_id=(SELECT resume_operation_id FROM zuse_rust_runtime_repairs WHERE operation_id=$1)").bind(operation).execute(&pool).await.unwrap();
    // A later authorized Sleep/Resume replaces the current pointer. The repair
    // must retain its exact durable ACK and still require current online state.
    sqlx::query("UPDATE api_cloud_workspaces SET state='ready',runtime_state='online',request_config=jsonb_set(jsonb_set(request_config,'{rustLifecycleOperation,phase}','\"ready\"'),'{rustLifecycleOperation,id}','\"later-user-resume\"') WHERE workspace_id='ws-bob'").execute(&pool).await.unwrap();
    sqlx::query("UPDATE zuse_rust_jobs SET due_at=0 WHERE kind=$1")
        .bind(JOB_KIND)
        .execute(&pool)
        .await
        .unwrap();
    control
        .runtime_repair_once(&provider, now + 8000)
        .await
        .unwrap();
    let attention = control
        .runtime_repair_status("ws-bob", operation, &bob)
        .await
        .unwrap();
    let marker_cleared:bool=sqlx::query_scalar("SELECT NOT(request_config?'rustMemoryTransportRecovery') FROM api_cloud_workspaces WHERE workspace_id='ws-bob'")
        .fetch_one(&pool).await.unwrap();
    assert!(
        marker_cleared,
        "only the matched new-runtime Ready ACK may clear transport attention"
    );
    assert_eq!(attention["state"], "attention-required");
    let lease = control
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
            now + 8001,
        )
        .await;
    assert_eq!(
        lease
            .err()
            .expect("attention must fence new execution")
            .code,
        "cloud_workspace_runtime_not_ready"
    );

    let late_ack: zuse_protocol::cloud_commands::RuntimeAcknowledgment=serde_json::from_value(json!({"commandId":"attempted","leaseToken":"retired-lease","fingerprint":"fingerprint","state":"applied"})).unwrap();
    let status = control
        .command_ack("ws-bob", "runtime-fixture", &late_ack, now + 8002)
        .await
        .unwrap();
    assert_eq!(
        status["state"], "outcome-unknown",
        "an old guest ACK cannot turn quarantined unknown work into Applied"
    );
    assert_eq!(attention["recovery"]["unknownServerCommandCount"], 1);
    let resolved = control
        .resolve_runtime_repair(
            "ws-bob",
            operation,
            &bob,
            Resolve {
                decision: "discard".into(),
            },
            now + 9000,
        )
        .await
        .unwrap();
    assert_eq!(resolved["state"], "ready");
    if !real {
        sqlx::query("UPDATE api_cloud_workspaces SET request_config=jsonb_set(request_config,'{rustRecreation}',$1) WHERE workspace_id='ws-bob'").bind(json!({"operationId":uuid::Uuid::new_v4().to_string(),"phase":"capturing"})).execute(&pool).await.unwrap();
        sqlx::query("UPDATE zuse_rust_jobs SET due_at=0 WHERE kind=$1")
            .bind(super::super::allocation::JOB_KIND)
            .execute(&pool)
            .await
            .unwrap();
        assert!(
            control
                .allocate_once(&NoAllocationRpc, now + 9000)
                .await
                .unwrap(),
            "recreation also holds original dispatch without RPC"
        );
        sqlx::query("UPDATE api_cloud_workspaces SET request_config=jsonb_set(request_config-'rustRecreation','{rustCompletedLaunchCommandId}','\"original-first-command\"') WHERE workspace_id='ws-bob'").execute(&pool).await.unwrap();
        sqlx::query("UPDATE zuse_rust_jobs SET due_at=0 WHERE kind=$1")
            .bind(super::super::allocation::JOB_KIND)
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(
            sqlx::query_scalar::<_, i32>("SELECT attempts FROM zuse_rust_jobs WHERE kind=$1")
                .bind(super::super::allocation::JOB_KIND)
                .fetch_one(&pool)
                .await
                .unwrap(),
            0,
            "held maintenance never exhausts provider retries"
        );
        sqlx::query("UPDATE zuse_rust_allocations SET machine_id='wrong-original-source' WHERE workspace_id='ws-bob'").execute(&pool).await.unwrap();
        assert_eq!(
            control
                .allocate_once(&NoAllocationRpc, now + 9050)
                .await
                .unwrap_err()
                .code,
            "cloud_allocation_identity_conflict"
        );
        sqlx::query(
            "UPDATE zuse_rust_allocations SET machine_id='machine-bob' WHERE workspace_id='ws-bob'",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("UPDATE zuse_rust_jobs SET due_at=0 WHERE kind=$1")
            .bind(super::super::allocation::JOB_KIND)
            .execute(&pool)
            .await
            .unwrap();
        assert!(
            control
                .allocate_once(&NoAllocationRpc, now + 9100)
                .await
                .unwrap(),
            "canonical completed launch finishes expired original allocation without replay"
        );
        assert_eq!(
            sqlx::query_scalar::<_, String>(
                "SELECT phase FROM zuse_rust_allocations WHERE workspace_id='ws-bob'"
            )
            .fetch_one(&pool)
            .await
            .unwrap(),
            "ready"
        );
        assert_eq!(sqlx::query_scalar::<_,String>("SELECT ciphertext FROM api_cloud_workspace_launch_intents WHERE workspace_id='ws-bob'").fetch_one(&pool).await.unwrap(),"opaque-original-intent");
    }

    assert_eq!(
        sqlx::query_scalar::<_, String>(
            "SELECT state FROM zuse_rust_mailbox_commands WHERE command_id='pending'"
        )
        .fetch_one(&pool)
        .await
        .unwrap(),
        "cancelled"
    );
    assert_eq!(
        sqlx::query_scalar::<_, String>(
            "SELECT provider_sandbox_id FROM api_cloud_workspaces WHERE workspace_id='ws-bob'"
        )
        .fetch_one(&pool)
        .await
        .unwrap(),
        "machine-bob"
    );
    assert_eq!(
        *provider.actions.lock().unwrap(),
        vec![
            "repair-stop",
            "repair-status",
            "repair-start",
            "repair-start"
        ]
    );
    if let Some(bridge) = &provider.bridge {
        bridge.lock().await.verify_and_close().await;
    }
    // Delayed repair is an execution operation: current membership is required
    // for both the immutable task author and the actor requesting repair.
    sqlx::raw_sql("CREATE TABLE api_github_enrollments(organization_id text,account_id text,installation_id bigint,data jsonb);CREATE TABLE api_github_join_policies(organization_id text,installation_id bigint,data jsonb)").execute(&pool).await.unwrap();
    sqlx::query("UPDATE api_cloud_workspaces SET account_id='organization:repair-team',desired_state='paused',request_config=(request_config-'managedCandidateSha256'-'managedCandidateArtifactId'-'managedBaseReadiness')||$1 WHERE workspace_id='ws-bob'").bind(json!({"rustActorId":"creator","rustOrganizationExecutionVersion":1,"providerAuthMode":"broker-v1","managedReleaseId":"release","agentConnectionId":"connection","agentConnectionOwnerId":"creator","sharingPolicy":{"audience":"private","permission":"view","creatorSubject":"creator","creatorMembershipId":"membership-creator","grants":[]},"rustRuntimeRepair":{"operationId":operation,"phase":"stopping","updatedAt":now+10000}})).execute(&pool).await.unwrap();
    sqlx::query("UPDATE zuse_rust_runtime_repairs SET phase='stopping' WHERE operation_id=$1")
        .bind(operation)
        .execute(&pool)
        .await
        .unwrap();
    let authority = std::sync::Arc::new(RepairScopeIdentity(AtomicBool::new(false)));
    control
        .configure_scope_authority(authority.clone(), None, vec!["repair-team".into()])
        .unwrap();
    let proof = control
        .signer
        .allocation_scope_proof("ws-bob", &format!("repair:{operation}"), 1, now + 10000)
        .unwrap();
    control
        .authorize_worker_effect_scope("ws-bob", &proof, now + 10000)
        .await
        .unwrap();
    authority.0.store(true, Ordering::SeqCst);
    control
        .invalidate_organization_access("repair-team", "user-bob")
        .await;
    assert_eq!(
        control
            .authorize_worker_effect_scope("ws-bob", &proof, now + 10001)
            .await
            .unwrap_err()
            .code,
        "organization_access_denied"
    );
    sqlx::query("UPDATE zuse_rust_jobs SET state='pending',attempts=0,due_at=0,lease_token=NULL,lease_expires_at=NULL WHERE kind=$1").bind(JOB_KIND).execute(&pool).await.unwrap();
    let before = provider.actions.lock().unwrap().len();
    assert_eq!(
        control
            .runtime_repair_once(&provider, now + 10002)
            .await
            .unwrap_err()
            .code,
        "organization_access_denied"
    );
    assert_eq!(
        provider.actions.lock().unwrap().len(),
        before,
        "departed requesting actor cannot dispatch a supervisor mutation"
    );
    drop(control);
    drop(provider);
    pool.close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .unwrap();
    admin.close().await;
}

struct RepairScopeIdentity(AtomicBool);
#[async_trait]
impl crate::auth::Identity for RepairScopeIdentity {
    async fn verify(&self, _: &str) -> Result<Principal, ApiError> {
        unreachable!()
    }
    async fn membership(
        &self,
        actor: &str,
        organization: &str,
    ) -> Result<crate::auth::Membership, ApiError> {
        if organization != "repair-team" || (actor == "user-bob" && self.0.load(Ordering::SeqCst)) {
            return Err(ApiError::forbidden("organization_access_denied"));
        }
        Ok(crate::auth::Membership {
            id: format!("membership-{actor}"),
            user_id: actor.into(),
            organization_id: organization.into(),
            status: "active".into(),
            role: crate::auth::MembershipRole {
                slug: if actor == "user-bob" {
                    "admin"
                } else {
                    "member"
                }
                .into(),
            },
        })
    }
}

struct RealSupervisorBridge {
    child: tokio::process::Child,
    input: tokio::process::ChildStdin,
    output: tokio::io::BufReader<tokio::process::ChildStdout>,
}
impl RealSupervisorBridge {
    async fn start() -> Self {
        let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../infra/cloud/tests/runtime-repair-bridge.py");
        let mut child = tokio::process::Command::new("python3")
            .arg(script)
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let input = child.stdin.take().unwrap();
        let output = tokio::io::BufReader::new(child.stdout.take().unwrap());
        let mut bridge = Self {
            child,
            input,
            output,
        };
        assert_eq!(
            bridge.request(&json!({"control":"prepare"})).await.unwrap()["prepared"],
            true
        );
        bridge
    }
    async fn request(&mut self, request: &Value) -> Result<Value, ProviderError> {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
        tokio::time::timeout(std::time::Duration::from_secs(8), async {
            let line = serde_json::to_string(request).unwrap() + "\n";
            self.input
                .write_all(line.as_bytes())
                .await
                .map_err(|_| ProviderError::Unavailable)?;
            self.input
                .flush()
                .await
                .map_err(|_| ProviderError::Unavailable)?;
            let mut line = String::new();
            self.output
                .read_line(&mut line)
                .await
                .map_err(|_| ProviderError::Unavailable)?;
            if line.len() > 65536 {
                return Err(ProviderError::InvalidResponse);
            }
            let response: Value =
                serde_json::from_str(&line).map_err(|_| ProviderError::InvalidResponse)?;
            if response["ok"] != true {
                return Err(ProviderError::Rejected);
            }
            Ok(response["value"].clone())
        })
        .await
        .map_err(|_| ProviderError::Unavailable)?
    }
    async fn verify_and_close(&mut self) {
        let report = self.request(&json!({"control":"verify"})).await.unwrap();
        assert_eq!(report["effects"], 1, "committed action must not run twice");
        assert_eq!(report["orphanRetired"], true);
        assert_eq!(report["dirtyRetained"], true);
        assert_eq!(report["childAlive"], true);
        assert_ne!(report["freshPid"], report["originalPid"]);
        assert_eq!(report["localState"], "outcome-unknown");
        self.request(&json!({"control":"quit"})).await.unwrap();
        assert!(self.child.wait().await.unwrap().success());
    }
}
impl Drop for RealSupervisorBridge {
    fn drop(&mut self) {
        if let Some(pid) = self.child.id() {
            unsafe {
                libc::kill(pid as i32, libc::SIGTERM);
            }
        }
    }
}

/// Real guest Authority HTTP transport sees no Resume after operator withdrawal,
/// while its original local extra remains true (credential renewal cannot fix it).
async fn assert_real_authority_withdrawal(control: &RuntimeControl, now: i64) {
    use std::sync::atomic::{AtomicBool, Ordering};
    let enabled = std::sync::Arc::new(AtomicBool::new(true));
    let signing = ed25519_dalek::SigningKey::from_bytes(&[48; 32]);
    let jwk = json!({"kty":"OKP","crv":"Ed25519","d":URL_SAFE_NO_PAD.encode([48;32]),"x":URL_SAFE_NO_PAD.encode(signing.verifying_key().as_bytes())});
    // The actual HTTP Query must reuse this sole checkout across candidate
    // validation and workspace locks; a nested acquire fails within two seconds.
    let schema: String = sqlx::query_scalar("SELECT current_schema()")
        .fetch_one(&control.pool)
        .await
        .unwrap();
    let options = (*control.pool.connect_options()).clone();
    let authority_pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .acquire_timeout(Duration::from_secs(2))
        .after_connect(move |connection, _| {
            let schema = schema.clone();
            Box::pin(async move {
                sqlx::query(&format!("SET search_path={schema}"))
                    .execute(connection)
                    .await?;
                Ok(())
            })
        })
        .connect_with(options)
        .await
        .unwrap();
    let route_control = std::sync::Arc::new(
        RuntimeControl::from_pool(
            authority_pool.clone(),
            MasterKey::parse(&URL_SAFE_NO_PAD.encode([49; 32])).unwrap(),
            TicketSigner::from_jwk(&jwk.to_string(), "https://api.example.test").unwrap(),
        )
        .with_staging_base_pin(control.staging_candidate.clone()),
    );
    let route_enabled = enabled.clone();
    let app = axum::Router::new().route(
        "/v1/cloud/workspaces/ws-bob/runtime/lifecycle",
        axum::routing::post(
            move |headers: axum::http::HeaderMap,
                  axum::Json(request): axum::Json<
                zuse_protocol::cloud_lifecycle::RuntimeRequest,
            >| {
                let control = route_control.clone();
                let enabled = route_enabled.clone();
                async move {
                    assert_eq!(
                        headers.get("authorization").unwrap(),
                        "Bearer runtime-fixture"
                    );
                    let owner = enabled.load(Ordering::SeqCst).then_some("user-bob");
                    axum::Json(
                        control
                            .lifecycle_runtime_pilot(
                                "ws-bob",
                                "runtime-fixture",
                                request,
                                now,
                                Some("staging"),
                                owner,
                            )
                            .await
                            .unwrap(),
                    )
                }
            },
        ),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let boot:zuse_protocol::cloud::CloudRuntimeBootstrap=serde_json::from_value(json!({"workspaceId":"ws-bob","zuseAccountId":"user-bob","chatId":"parent","initialSessionId":"child","runtimeCredential":"runtime-fixture","runtimeGeneration":1,"gatewayEpoch":1,"runtimeCredentialExpiresAt":now+3600000,"gatewayUrl":"ws://127.0.0.1/gateway","gatewayProtocol":zuse_protocol::cloud_gateway::RUST_PROTOCOL,"rpcProtocol":zuse_protocol::cloud_gateway::RUST_RPC,"sealedTranscriptKey":"unused","runtimeStorage":{"format":"zuse-rust-engine-v1","storageIncarnationId":"disk"},"internalCodexMemoryTransportReset":true})).unwrap();
    let authority = zuse_cloud_runtime::Authority::new(&format!("http://{address}")).unwrap();
    assert!(
        authority
            .lifecycle(&boot, zuse_protocol::cloud_lifecycle::RuntimeRequest::Query)
            .await
            .unwrap()
            .is_some()
    );
    let admitted:Value=sqlx::query_scalar("SELECT request_config->'rustMemoryTransportAdmission' FROM api_cloud_workspaces WHERE workspace_id='ws-bob'").fetch_one(&control.pool).await.unwrap();
    assert_eq!(
        admitted["enabled"], true,
        "actual current-pilot Query backfills old retained bootstrap admission"
    );
    enabled.store(false, Ordering::SeqCst);
    assert_eq!(
        boot.extra["internalCodexMemoryTransportReset"], true,
        "guest still retains its original extra; do not fake a refresh"
    );
    assert!(
        authority
            .lifecycle(&boot, zuse_protocol::cloud_lifecycle::RuntimeRequest::Query)
            .await
            .unwrap()
            .is_none(),
        "no Resume reaches fe94 before privileged reset"
    );
    server.abort();
    let _ = server.await;
    authority_pool.close().await;
}

struct NoAllocationRpc;
#[async_trait]
impl super::super::allocation::AllocationProvider for NoAllocationRpc {
    async fn find(&self, _: &str) -> Result<Option<Machine>, ProviderError> {
        panic!("held original allocation must not inspect/find a provider resource")
    }
    async fn create(&self, _: &str, _: &str) -> Result<Machine, ProviderError> {
        panic!("held allocation must not create")
    }
    async fn launch(
        &self,
        _: &str,
        _: &super::super::allocation::RuntimeLaunch,
    ) -> Result<(), ProviderError> {
        panic!("held allocation must not replay original launch")
    }
}
