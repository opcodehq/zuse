use super::*;
use crate::{crypto::MasterKey, tickets::TicketSigner};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};
struct Probe {
    pool: sqlx::PgPool,
    workspace: String,
    mode: String,
    boat: bool,
    machine: Mutex<Option<Machine>>,
    creates: AtomicUsize,
    calls: AtomicUsize,
    starts: AtomicUsize,
    uploads: AtomicUsize,
    original: Mutex<Option<String>>,
}
#[async_trait]
impl AllocationProvider for Probe {
    fn provider_id(&self) -> &'static str {
        if self.boat { "boat" } else { "boxd" }
    }
    async fn find(&self, _: &str) -> Result<Option<Machine>, ProviderError> {
        Ok(self.machine.lock().unwrap().clone())
    }
    async fn create(&self, source: &str, name: &str) -> Result<Machine, ProviderError> {
        assert_eq!(source, "managed-artifact");
        self.creates.fetch_add(1, Ordering::SeqCst);
        let m = Machine {
            id: "original-fresh-target".into(),
            name: name.into(),
            state: MachineState::Running,
            vcpu: None,
            memory_bytes: None,
        };
        *self.machine.lock().unwrap() = Some(m.clone());
        if self.mode == "already-bound" {
            // Inject before the positive receipt's immediate boot stage.
            sqlx::query("UPDATE api_cloud_workspaces SET runtime_credential_hash='bound' WHERE workspace_id=$1").bind(&self.workspace).execute(&self.pool).await.unwrap();
        }
        Ok(m)
    }
    async fn bootstrap_initial_runtime(
        &self,
        machine: &str,
        source: &str,
        name: &str,
        r: &Request,
    ) -> Result<Option<Value>, ProviderError> {
        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            sqlx::query(
                "UPDATE api_cloud_workspaces SET revision=revision+1 WHERE workspace_id=$1",
            )
            .bind(&self.workspace)
            .execute(&self.pool),
        )
        .await
        .expect("No SQL lock across image bootstrap")
        .unwrap();
        assert_eq!(machine, "original-fresh-target");
        assert_eq!(source, "managed-artifact");
        assert_eq!(name, self.machine.lock().unwrap().as_ref().unwrap().name);
        assert_eq!(r.source_machine_id, machine);
        assert_eq!(r.workspace_id, self.workspace);
        assert!(r.purpose == Purpose::InitialAllocation);
        let digest = r.digest().unwrap();
        {
            let mut original = self.original.lock().unwrap();
            if let Some(prior) = original.as_ref() {
                assert_eq!(prior, &digest)
            } else {
                *original = Some(digest.clone());
                self.starts.fetch_add(1, Ordering::SeqCst);
            }
        }
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        let sql = match self.mode.as_str() {
            "delete" => Some(
                "UPDATE api_cloud_workspaces SET desired_state='deleted',state='deleting' WHERE workspace_id=$1",
            ),
            "lease-expired" => {
                Some("UPDATE zuse_rust_jobs SET lease_expires_at=0 WHERE resource_id=$1")
            }
            "lease-replaced" => {
                Some("UPDATE zuse_rust_jobs SET lease_token='new-lease' WHERE resource_id=$1")
            }
            "generation" => Some(
                "UPDATE zuse_rust_control_owners SET generation=generation+1 WHERE resource_id=$1",
            ),
            "disk" => Some(
                "UPDATE api_cloud_workspaces SET request_config=jsonb_set(request_config,'{rustRuntimeStorage,storageIncarnationId}','\"different-disk\"') WHERE workspace_id=$1",
            ),
            "source" => Some(
                "UPDATE zuse_rust_allocations SET snapshot_id='different-source' WHERE workspace_id=$1",
            ),
            "operation" => Some(
                "UPDATE api_cloud_workspaces SET request_config=jsonb_set(request_config,'{rustAllocationOperationId}','\"different-operation\"') WHERE workspace_id=$1",
            ),
            "pins" => Some(
                "UPDATE api_cloud_workspaces SET request_config=jsonb_set(request_config,'{initialRuntimeBootstrap,helperSha256}',to_jsonb(repeat('f',64))) WHERE workspace_id=$1",
            ),
            "expiry" => {
                Some("UPDATE zuse_rust_allocations SET boot_expires_at=0 WHERE workspace_id=$1")
            }
            _ => None,
        };
        if let Some(sql) = sql {
            sqlx::query(sql)
                .bind(&self.workspace)
                .execute(&self.pool)
                .await
                .unwrap();
        }
        if matches!(self.mode.as_str(), "pending" | "pending-fast") && call == 0 {
            return Ok(None);
        }
        if self.mode == "reply-lost" && call == 0 {
            return Err(ProviderError::OutcomeUnknown);
        }
        let mut response = serde_json::to_value(r).unwrap();
        response["status"] = json!("ready");
        response["requestSha256"] = json!(digest);
        response["unitStartSubmitted"] = json!(true);
        response["resident"] = json!({"residentPid":42,"residentStartTicks":1,"bootNonceSha256":"a".repeat(64),"ipcVerified":true,"unbound":true,"childAlive":false,"recoveryAlive":false});
        r.response(&response)
    }
    async fn launch(&self, _: &str, launch: &RuntimeLaunch) -> Result<(), ProviderError> {
        assert!(!launch.boot_token.is_empty());
        let row: Value = sqlx::query_scalar(
            "SELECT request_config FROM api_cloud_workspaces WHERE workspace_id=$1",
        )
        .bind(&self.workspace)
        .fetch_one(&self.pool)
        .await
        .unwrap();
        if self.mode != "legacy" {
            assert!(
                row["initialRuntimeBootstrapReceipt"]["proof"]["ipcVerified"] == true,
                "Proof must commit BEFORE private boot upload"
            );
            assert_eq!(
                row["initialRuntimeBootstrapReceipt"]["sourceArtifactId"],
                "managed-artifact"
            );
        }
        self.uploads.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}
#[tokio::test]
#[ignore = "requires isolated PostgreSQL"]
async fn fresh_image_boot_is_opt_in_idempotent_and_fenced_before_any_credential_upload() {
    let url = std::env::var("ZUSE_CLOUD_TEST_DATABASE_URL").expect("isolated test database");
    assert!(
        matches!(
            reqwest::Url::parse(&url).unwrap().host_str(),
            Some("localhost" | "127.0.0.1" | "::1")
        ),
        "isolated localhost database required"
    );
    let admin = sqlx::PgPool::connect(&url).await.unwrap();
    for batched in [false, true] {
        for variant in [
            "boat-ready",
            "boat-reply-lost",
            "boat-pending",
            "boat-lease-expired",
            "boat-lease-replaced",
            "boat-delete",
            "boat-generation",
            "boat-disk",
            "boat-source",
            "boat-operation",
            "boat-pins",
            "boat-expiry",
            "boat-already-bound",
            "boat-legacy",
            "ready",
            "pending-fast",
            "legacy",
            "pending",
            "reply-lost",
            "delete",
            "lease-expired",
            "lease-replaced",
            "generation",
            "disk",
            "source",
            "operation",
            "pins",
            "expiry",
            "already-bound",
        ] {
            let boat = variant.starts_with("boat-");
            let mode = variant.strip_prefix("boat-").unwrap_or(variant);
            let schema = format!("fresh_boot_{}", uuid::Uuid::new_v4().simple());
            sqlx::query(&format!("CREATE SCHEMA {schema}"))
                .execute(&admin)
                .await
                .unwrap();
            let scope = schema.clone();
            let pool = sqlx::postgres::PgPoolOptions::new()
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
            for script in [
                include_str!("../../../../tests/fixtures/legacy-catalog.sql"),
                jobs::MIGRATION,
                super::super::MIGRATION,
                crate::runtime::deletion::MIGRATION,
            ] {
                sqlx::raw_sql(script).execute(&pool).await.unwrap();
            }
            if batched {
                crate::runtime::statement_batch::fixture_schema(&pool).await;
            }
            let workspace = format!("ws-{}", uuid::Uuid::new_v4().simple());
            let mut config = json!({"runtimeGeneration":1,"gatewayEpoch":1,"rustProviderControllerVersion":1,"managedReleaseId":"explicit-fresh-release"});
            if mode == "pending-fast" {
                config["rustFastStartVersion"] = json!(1);
            }
            if batched && !boat && mode != "pending" {
                config["rustFastStartVersion"] = json!(1);
                config["agent"] = json!("codex");
            }
            if mode != "legacy" {
                config["initialRuntimeBootstrap"] = json!({"version":1,"mode":if boat {"post-hydration-first-start-v1"} else {"stopped-memory-snapshot-v1"},"runtimeSha256":"a".repeat(64),"supervisorSha256":"b".repeat(64),"serviceSha256":"c".repeat(64),"manifestSha256":"d".repeat(64),"helperSha256":"e".repeat(64)});
            }
            sqlx::query("UPDATE api_cloud_workspaces SET workspace_id=$1,provider=$3,provider_sandbox_id=NULL,runtime_credential_hash=NULL,runtime_boot_token_hash=NULL,state='queued',desired_state='ready',runtime_state='offline',request_config=$2 WHERE workspace_id='ws-bob'").bind(&workspace).bind(config).bind(if boat {"boat"} else {"boxd"}).execute(&pool).await.unwrap();
            let now = chrono::Utc::now().timestamp_millis();
            sqlx::query("INSERT INTO zuse_rust_control_owners VALUES('workspace',$1,'rust',1,$2)")
                .bind(&workspace)
                .bind(now)
                .execute(&pool)
                .await
                .unwrap();
            sqlx::query("INSERT INTO api_cloud_workspace_launch_intents VALUES($1,'user-bob','chat','session','turn','command','test-ciphertext',$2,$3)").bind(&workspace).bind(now+86400000).bind(now).execute(&pool).await.unwrap();
            let signing = ed25519_dalek::SigningKey::from_bytes(&[74; 32]);
            let jwk = json!({"kty":"OKP","crv":"Ed25519","d":URL_SAFE_NO_PAD.encode([74;32]),"x":URL_SAFE_NO_PAD.encode(signing.verifying_key().as_bytes())});
            let control = RuntimeControl::from_pool(
                pool.clone(),
                MasterKey::parse(&URL_SAFE_NO_PAD.encode([75; 32])).unwrap(),
                TicketSigner::from_jwk(&jwk.to_string(), "https://api.example.test").unwrap(),
            );
            let control = control.with_fast_start_accounts(if batched {
                vec!["user-bob".into()]
            } else {
                vec![]
            });
            assert_eq!(control.statement_batch_available().await.unwrap(), batched);
            let operation = control
                .queue_allocation(&workspace, "managed-artifact", now)
                .await
                .unwrap();
            let probe = Arc::new(Probe {
                pool: pool.clone(),
                workspace: workspace.clone(),
                mode: mode.into(),
                boat,
                machine: Mutex::new(None),
                creates: AtomicUsize::new(0),
                calls: AtomicUsize::new(0),
                starts: AtomicUsize::new(0),
                uploads: AtomicUsize::new(0),
                original: Mutex::new(None),
            });
            // Positive create and initial boot are one bounded worker call now.
            // The fixture must not manually force an unrelated second launch first.
            let result = control
                .allocate_once(&*probe, chrono::Utc::now().timestamp_millis())
                .await;
            let success = matches!(mode, "ready" | "legacy" | "pending-fast");
            if success {
                assert!(result.is_ok(), "{mode}: {result:?}");
                assert_eq!(probe.uploads.load(Ordering::SeqCst), 1);
            } else {
                assert_eq!(probe.uploads.load(Ordering::SeqCst), 0, "{mode}");
                if mode != "pending" {
                    assert!(result.is_err(), "{mode}")
                }
            }
            if matches!(mode, "pending" | "reply-lost") {
                sqlx::query("UPDATE zuse_rust_jobs SET due_at=0 WHERE resource_id=$1")
                    .bind(&workspace)
                    .execute(&pool)
                    .await
                    .unwrap();
                control
                    .allocate_once(&*probe, chrono::Utc::now().timestamp_millis())
                    .await
                    .unwrap();
                assert_eq!(probe.uploads.load(Ordering::SeqCst), 1);
                assert_eq!(probe.starts.load(Ordering::SeqCst), 1);
                assert_eq!(probe.calls.load(Ordering::SeqCst), 2);
            }
            if mode == "ready" {
                sqlx::query("UPDATE api_cloud_workspaces SET runtime_credential_hash='now-bound' WHERE workspace_id=$1").bind(&workspace).execute(&pool).await.unwrap();
                sqlx::query("UPDATE zuse_rust_jobs SET due_at=0 WHERE resource_id=$1")
                    .bind(&workspace)
                    .execute(&pool)
                    .await
                    .unwrap();
                control
                    .allocate_once(&*probe, chrono::Utc::now().timestamp_millis())
                    .await
                    .unwrap();
                assert_eq!(
                    probe.calls.load(Ordering::SeqCst),
                    1,
                    "Saved original receipt must avoid helper on now-bound retry"
                );
            }
            assert_eq!(probe.creates.load(Ordering::SeqCst), 1);
            if matches!(mode, "legacy" | "already-bound") {
                assert_eq!(probe.calls.load(Ordering::SeqCst), 0)
            }
            let id: String = sqlx::query_scalar(
                "SELECT machine_id FROM zuse_rust_allocations WHERE operation_id=$1",
            )
            .bind(&operation)
            .fetch_one(&pool)
            .await
            .unwrap();
            assert_eq!(id, "original-fresh-target");
            pool.close().await;
            sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
                .execute(&admin)
                .await
                .unwrap();
        }
    }
    admin.close().await;
}

#[test]
fn positive_create_authority_requires_every_exact_server_owned_receipt_binding() {
    let request: Request = serde_json::from_value(json!({"version":1,"provider":"boat","purpose":"initial-allocation","operationId":"operation-positive","workspaceId":"workspace-positive","generation":3,"storageIncarnationId":"storage-positive","sourceMachineId":"bx_positive","imagePins":{"runtimeSha256":"a".repeat(64),"supervisorSha256":"b".repeat(64),"serviceSha256":"c".repeat(64),"manifestSha256":"d".repeat(64),"helperSha256":"e".repeat(64)}})).unwrap();
    assert!(request.valid());
    let row = json!({"request_config":{"rustPositiveCreateReceipt":positive_create_source(&request,"snapshot-positive","name-positive")}});
    let authority =
        verified_create_source(&row, &request, "snapshot-positive", "name-positive").unwrap();
    assert!(authority.matches_request(&request, "snapshot-positive", "name-positive"));
    assert!(authority.matches_boat_source("bx_positive", "snapshot-positive", "name-positive"));
    for key in [
        "provider",
        "operationId",
        "workspaceId",
        "generation",
        "sourceArtifactId",
        "machineName",
        "machineId",
        "storageIncarnationId",
        "imagePins",
    ] {
        let mut forged = row.clone();
        forged["request_config"]["rustPositiveCreateReceipt"][key] = json!("forged");
        assert!(
            verified_create_source(&forged, &request, "snapshot-positive", "name-positive")
                .is_none(),
            "{key}"
        );
    }
    assert!(
        verified_create_source(&json!({}), &request, "snapshot-positive", "name-positive")
            .is_none()
    );
    assert!(!authority.matches_boat_source(
        "unrelated-machine",
        "snapshot-positive",
        "name-positive"
    ));
    let mut stale = request.clone();
    stale.generation += 1;
    assert!(!authority.matches_request(&stale, "snapshot-positive", "name-positive"));
}
