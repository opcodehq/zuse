use super::*;
use crate::{
    crypto::MasterKey,
    jobs,
    providers::{ExecResult, contract::*, registry::Registry},
    runtime::RuntimeControl,
    tickets::TicketSigner,
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use serde_json::json;
use sqlx::PgPool;
use std::sync::{
    Mutex,
    atomic::{AtomicUsize, Ordering},
};
use zuse_protocol::cloud_computers::ProviderId;

struct Fixture {
    id: ProviderId,
    pool: PgPool,
    workspace: String,
    operation: Mutex<String>,
    machine: Mutex<Option<Machine>>,
    finds: AtomicUsize,
    creates: AtomicUsize,
    uploads: AtomicUsize,
    inspections: AtomicUsize,
    launches: AtomicUsize,
    seen_leases: Mutex<Vec<(String, String, i64)>>,
    token: Mutex<Vec<u8>>,
    readiness: Mutex<String>,
}
impl Fixture {
    async fn assert_unlocked(&self) {
        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            sqlx::query(
                "UPDATE api_cloud_workspaces SET revision=revision+1 WHERE workspace_id=$1",
            )
            .bind(&self.workspace)
            .execute(&self.pool),
        )
        .await
        .expect("Provider calls must not retain the workspace lock")
        .unwrap();
        let lease:Option<(String,String,i64)>=sqlx::query_as("SELECT job_id,lease_token,resource_generation FROM zuse_rust_jobs WHERE resource_id=$1 AND state='running'").bind(&self.workspace).fetch_optional(&self.pool).await.unwrap();
        if let Some(lease) = lease {
            self.seen_leases.lock().unwrap().push(lease);
        }
    }
}
#[async_trait]
impl Provider for Fixture {
    fn id(&self) -> ProviderId {
        self.id
    }
    async fn inspect(&self, id: &str) -> Result<Option<Machine>, ProviderError> {
        self.assert_unlocked().await;
        assert_eq!(id, "original-resource");
        self.inspections.fetch_add(1, Ordering::SeqCst);
        Ok(self.machine.lock().unwrap().clone())
    }
    async fn runtime_ready(&self, id: &str) -> Result<bool, ProviderError> {
        self.assert_unlocked().await;
        assert_eq!(id, "original-resource");
        let mode = self.readiness.lock().unwrap().clone();
        let mode = mode.strip_prefix("immediate-").unwrap_or(&mode);
        if mode == "pending" {
            *self.readiness.lock().unwrap() = "ready".into();
            return Ok(false);
        }
        let statement = match mode {
            "delete" => Some(
                "UPDATE api_cloud_workspaces SET desired_state='deleted' WHERE workspace_id=$1",
            ),
            "generation" => Some(
                "UPDATE zuse_rust_control_owners SET generation=generation+1 WHERE resource_id=$1",
            ),
            "lease" => Some(
                "UPDATE zuse_rust_jobs SET lease_expires_at=0 WHERE resource_id=$1 AND state='running'",
            ),
            "source" => Some(
                "UPDATE api_cloud_workspaces SET provider_sandbox_id='replacement' WHERE workspace_id=$1",
            ),
            "wrong-operation" => Some(
                "UPDATE api_cloud_workspaces SET request_config=jsonb_set(request_config,'{rustAllocationOperationId}','\"other\"'::jsonb) WHERE workspace_id=$1",
            ),
            _ => None,
        };
        if let Some(statement) = statement {
            sqlx::query(statement)
                .bind(&self.workspace)
                .execute(&self.pool)
                .await
                .unwrap();
        }
        Ok(true)
    }
    async fn find_allocation(&self, _: &str) -> Result<Option<Machine>, ProviderError> {
        self.assert_unlocked().await;
        let n = self.finds.fetch_add(1, Ordering::SeqCst);
        // Immediate fixtures have a positive first receipt; existing cases
        // exercise the original uncertain response/eventual lookup path.
        let immediate = self.readiness.lock().unwrap().starts_with("immediate-");
        Ok(if n < 2 && !immediate {
            None
        } else {
            self.machine.lock().unwrap().clone()
        })
    }
    async fn allocate(&self, r: &Allocation<'_>) -> Result<Machine, ProviderError> {
        self.assert_unlocked().await;
        self.creates.fetch_add(1, Ordering::SeqCst);
        assert_eq!(r.operation_id, self.operation.lock().unwrap().as_str());
        assert_eq!(r.size, "default");
        assert_eq!(r.artifact, "managed-artifact");
        let machine = Machine {
            id: "original-resource".into(),
            name: r.name.into(),
            state: MachineState::Running,
            vcpu: None,
            memory_bytes: None,
        };
        let mode = self.readiness.lock().unwrap().clone();
        if mode == "immediate-rejected" {
            return Err(ProviderError::Rejected);
        }
        let mut receipt = machine.clone();
        if mode.starts_with("immediate-starting-") {
            receipt.state = MachineState::Starting;
        }
        *self.machine.lock().unwrap() = Some(if mode == "immediate-starting-pending" {
            receipt.clone()
        } else {
            machine
        });
        if mode.starts_with("immediate-") && mode != "immediate-uncertain" {
            Ok(receipt)
        } else {
            Err(ProviderError::OutcomeUnknown)
        }
    }
    async fn sleep(&self, _: &str) -> Result<ProviderOperation, ProviderError> {
        panic!("Allocation must not sleep a resource")
    }
    async fn resume(&self, _: &str, _: u32) -> Result<ProviderOperation, ProviderError> {
        panic!("Allocation must not resume a resource")
    }
    async fn delete(&self, _: &str) -> Result<ProviderOperation, ProviderError> {
        panic!("Allocation must not delete a resource")
    }
    async fn operation(&self, _: &ProviderOperation) -> Result<OperationState, ProviderError> {
        panic!("No lifecycle operation was admitted")
    }
    async fn execute(&self, id: &str, command: &str) -> Result<ExecResult, ProviderError> {
        self.assert_unlocked().await;
        assert_eq!(id, "original-resource");
        let token = self.token.lock().unwrap();
        assert!(!token.is_empty());
        assert!(
            !command.contains(std::str::from_utf8(&token).unwrap()),
            "Boot credentials must be uploaded privately rather than embedded in commands"
        );
        assert!(command.contains("launch-rust-runtime"));
        self.launches.fetch_add(1, Ordering::SeqCst);
        Ok(ExecResult {
            stdout: vec![],
            stderr: vec![],
            exit_code: 0,
        })
    }
    async fn upload(&self, id: &str, path: &str, data: &[u8]) -> Result<(), ProviderError> {
        self.assert_unlocked().await;
        assert_eq!(id, "original-resource");
        assert!(path.starts_with("/tmp/zuse-rust-boot-"));
        self.uploads.fetch_add(1, Ordering::SeqCst);
        *self.token.lock().unwrap() = data.to_vec();
        Ok(())
    }
}

#[tokio::test]
#[ignore = "requires explicit isolated localhost PostgreSQL"]
async fn all_provider_dispatch_preserves_allocation_identity_through_uncertain_and_eventually_consistent_replies()
 {
    let url = std::env::var("ZUSE_CLOUD_TEST_DATABASE_URL").expect("isolated database required");
    assert!(matches!(
        reqwest::Url::parse(&url).unwrap().host_str(),
        Some("127.0.0.1" | "localhost" | "::1")
    ));
    let admin = PgPool::connect(&url).await.unwrap();
    let cases = ProviderId::ALL.into_iter().map(|id| (id, "ready")).chain(
        [
            "pending",
            "delete",
            "generation",
            "lease",
            "source",
            "wrong-operation",
        ]
        .into_iter()
        .map(|mode| (ProviderId::Boat, mode)),
    );
    for (id, mode) in cases {
        let unique = uuid::Uuid::new_v4().simple().to_string();
        let schema = format!("allocation_{unique}");
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
        for sql in [
            include_str!("../../../tests/fixtures/legacy-catalog.sql"),
            jobs::MIGRATION,
            crate::runtime::allocation::MIGRATION,
            crate::runtime::deletion::MIGRATION,
        ] {
            sqlx::raw_sql(sql).execute(&pool).await.unwrap();
        }
        let workspace = format!("ws-{unique}");
        sqlx::query("UPDATE api_cloud_workspaces SET workspace_id=$1,provider=$2,provider_sandbox_id=NULL,runtime_credential_hash=NULL,runtime_boot_token_hash=NULL,state='queued',desired_state='ready',runtime_state='offline',request_config=$3 WHERE workspace_id='ws-bob'")
            .bind(&workspace).bind(id.as_str()).bind(json!({"runtimeGeneration":1,"gatewayEpoch":1,"rustProviderControllerVersion":1})).execute(&pool).await.unwrap();
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
        let operation = control
            .queue_allocation(&workspace, "managed-artifact", now)
            .await
            .unwrap();
        let fixture = Arc::new(Fixture {
            id,
            pool: pool.clone(),
            workspace: workspace.clone(),
            operation: Mutex::new(operation),
            machine: Mutex::new(None),
            finds: AtomicUsize::new(0),
            creates: AtomicUsize::new(0),
            uploads: AtomicUsize::new(0),
            inspections: AtomicUsize::new(0),
            launches: AtomicUsize::new(0),
            seen_leases: Mutex::new(vec![]),
            token: Mutex::new(vec![]),
            readiness: Mutex::new(mode.into()),
        });
        let mut registry = Registry::default();
        registry.insert(fixture.clone()).unwrap();
        // A confirmed reconciliation now continues the SAME claimed job once;
        // the initial unknown reply and empty reconciliation still cannot create again.
        for step in 0..3 {
            sqlx::query("UPDATE zuse_rust_jobs SET due_at=0 WHERE resource_id=$1")
                .bind(&workspace)
                .execute(&pool)
                .await
                .unwrap();
            let result = control
                .allocate_registry_once(&registry, chrono::Utc::now().timestamp_millis())
                .await;
            let blocked = !matches!(mode, "ready" | "pending") && step == 2;
            assert_eq!(
                result.is_err(),
                step < 2 || blocked,
                "{id:?}/{mode} step {step}: {result:?}"
            );
        }
        assert_eq!(fixture.creates.load(Ordering::SeqCst), 1);
        if mode == "pending" {
            assert_eq!(
                fixture.uploads.load(Ordering::SeqCst),
                0,
                "no boot credential before hydration"
            );
            sqlx::query("UPDATE zuse_rust_jobs SET due_at=0 WHERE resource_id=$1")
                .bind(&workspace)
                .execute(&pool)
                .await
                .unwrap();
            control
                .allocate_registry_once(&registry, chrono::Utc::now().timestamp_millis())
                .await
                .unwrap();
        }
        assert_eq!(
            fixture.uploads.load(Ordering::SeqCst),
            usize::from(matches!(mode, "ready" | "pending")),
            "{id:?}/{mode}"
        );
        let receipt: (String, String) =
            sqlx::query_as("SELECT outcome,machine_id FROM zuse_rust_provider_creates")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(receipt, ("confirmed".into(), "original-resource".into()));
        if matches!(mode, "ready" | "pending") {
            let phases: Value = sqlx::query_scalar("SELECT request_config#>'{startupTimings,providerPhases}' FROM api_cloud_workspaces WHERE workspace_id=$1")
                .bind(&workspace).fetch_one(&pool).await.unwrap();
            assert!(
                phases.get("providerCreate").is_none(),
                "an uncertain failed attempt without a persisted observation must not become a fabricated successful create timer"
            );
            assert_eq!(phases["providerLookup"]["callCount"], 1);
            assert_eq!(phases["bootCredentialUpload"]["callCount"], 1);
            assert_eq!(phases["runtimeLaunchCommand"]["lastResultOk"], true);
            assert!(phases["runtimeLaunchCommand"]["lastDurationMs"].is_u64());
            assert!(
                phases.get("launchArtifactVerification").is_none(),
                "unbound fixtures cannot fabricate artifact verification time"
            );
            assert!(!phases.to_string().contains("boot-token"));
        }

        drop(registry);
        drop(fixture);
        drop(control);
        pool.close().await;
        sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
            .execute(&admin)
            .await
            .unwrap();
    }
}

struct RecoveryFixture {
    id: ProviderId,
    uploads: Mutex<Vec<(String, String, Vec<u8>)>>,
    commands: Mutex<Vec<(String, String)>>,
}
#[async_trait]
impl Provider for RecoveryFixture {
    fn id(&self) -> ProviderId {
        self.id
    }
    async fn inspect(&self, id: &str) -> Result<Option<Machine>, ProviderError> {
        Ok(Some(Machine {
            id: id.into(),
            name: "retained".into(),
            state: MachineState::Running,
            vcpu: None,
            memory_bytes: None,
        }))
    }
    async fn find_allocation(&self, _: &str) -> Result<Option<Machine>, ProviderError> {
        panic!("recovery uses immutable resource IDs")
    }
    async fn allocate(&self, _: &Allocation<'_>) -> Result<Machine, ProviderError> {
        panic!("capture must not allocate")
    }
    async fn sleep(&self, id: &str) -> Result<ProviderOperation, ProviderError> {
        Ok(ProviderOperation {
            resource_id: id.into(),
            operation_id: Some("original-stop".into()),
            desired: DesiredState::Sleeping,
        })
    }
    async fn resume(&self, id: &str, _: u32) -> Result<ProviderOperation, ProviderError> {
        Ok(ProviderOperation {
            resource_id: id.into(),
            operation_id: Some("original-wake".into()),
            desired: DesiredState::Running,
        })
    }
    async fn delete(&self, id: &str) -> Result<ProviderOperation, ProviderError> {
        Ok(ProviderOperation {
            resource_id: id.into(),
            operation_id: Some("original-delete".into()),
            desired: DesiredState::Deleted,
        })
    }
    async fn operation(&self, r: &ProviderOperation) -> Result<OperationState, ProviderError> {
        assert_eq!(r.resource_id, "source");
        assert_eq!(
            r.operation_id.as_deref(),
            Some(match r.desired {
                DesiredState::Sleeping => "original-stop",
                DesiredState::Running => "original-wake",
                DesiredState::Deleted => "original-delete",
            })
        );
        Ok(OperationState::Retrying)
    }
    async fn upload(&self, id: &str, path: &str, data: &[u8]) -> Result<(), ProviderError> {
        self.uploads
            .lock()
            .unwrap()
            .push((id.into(), path.into(), data.to_vec()));
        Ok(())
    }
    async fn execute(&self, id: &str, command: &str) -> Result<ExecResult, ProviderError> {
        assert!(!command.contains("fixture-private-archive-capability"));
        self.commands
            .lock()
            .unwrap()
            .push((id.into(), command.into()));
        Ok(ExecResult {
            exit_code: 0,
            stdout: if command.contains("then cat --") {
                b"workspace_archive_restored\n".to_vec()
            } else {
                vec![]
            },
            stderr: vec![],
        })
    }
}
#[tokio::test]
async fn all_recovery_adapters_keep_capabilities_private_and_preserve_original_receipts() {
    use crate::runtime::{
        recovery::{CapturePlan, RecoveryProvider},
        restore::RestoreProvider,
    };
    for id in ProviderId::ALL {
        let remote = Arc::new(RecoveryFixture {
            id,
            uploads: Mutex::new(vec![]),
            commands: Mutex::new(vec![]),
        });
        let adapter = RuntimeProvider::new(remote.clone(), "default".into());
        assert_eq!(RecoveryProvider::provider_id(&adapter), id.as_str());
        let plan = CapturePlan {
            operation_id: "archive-id".into(),
            workspace_id: "task-id".into(),
            source_machine_id: "source".into(),
            config: zeroize::Zeroizing::new("fixture-private-archive-capability".into()),
        };
        RecoveryProvider::capture(&adapter, &plan).await.unwrap();
        RecoveryProvider::stop_capture(&adapter, "source", "archive-id")
            .await
            .unwrap();
        RestoreProvider::restore(&adapter, "replacement", &plan)
            .await
            .unwrap();
        assert!(
            RestoreProvider::installed(&adapter, "replacement", "archive-id")
                .await
                .unwrap()
        );
        let uploads = remote.uploads.lock().unwrap();
        assert_eq!(uploads.len(), 2);
        assert_eq!(uploads[0].0, "source");
        assert_eq!(uploads[1].0, "replacement");
        for (_, path, bytes) in uploads.iter() {
            assert_eq!(path, "/tmp/zuse-recovery-archive-id");
            assert_eq!(bytes, b"fixture-private-archive-capability");
        }
        drop(uploads);
        assert_eq!(remote.commands.lock().unwrap().len(), 4);
        for receipt in [
            RecoveryProvider::wake_receipt(&adapter, "source", &MachineState::Paused)
                .await
                .unwrap(),
            RecoveryProvider::pause_receipt(&adapter, "source")
                .await
                .unwrap(),
            RecoveryProvider::retire_receipt(&adapter, "source")
                .await
                .unwrap(),
        ] {
            let restored: ProviderOperation =
                serde_json::from_value(serde_json::to_value(&receipt).unwrap()).unwrap();
            assert_eq!(
                RecoveryProvider::recovery_state(&adapter, &restored)
                    .await
                    .unwrap(),
                OperationState::Retrying
            );
            if restored.desired == DesiredState::Deleted {
                assert_eq!(
                    RecoveryProvider::retirement_state(&adapter, &restored)
                        .await
                        .unwrap(),
                    OperationState::Retrying
                );
            }
        }
    }
}

struct BoundArtifactFixture {
    calls: Mutex<Vec<&'static str>>,
    bad_version: bool,
    bad_bytes: bool,
}
#[async_trait]
impl Provider for BoundArtifactFixture {
    fn id(&self) -> ProviderId {
        ProviderId::E2b
    }
    async fn inspect(&self, _: &str) -> Result<Option<Machine>, ProviderError> {
        unreachable!()
    }
    async fn find_allocation(&self, _: &str) -> Result<Option<Machine>, ProviderError> {
        unreachable!()
    }
    async fn allocate(&self, _: &Allocation<'_>) -> Result<Machine, ProviderError> {
        unreachable!()
    }
    async fn verify_artifact(
        &self,
        artifact: &str,
        binding: Option<&crate::providers::artifact::ArtifactBinding>,
    ) -> Result<(), ProviderError> {
        self.calls.lock().unwrap().push("verify-version");
        binding.unwrap().validate(ProviderId::E2b, artifact)?;
        if self.bad_version {
            Err(ProviderError::Rejected)
        } else {
            Ok(())
        }
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
    async fn execute(&self, _: &str, command: &str) -> Result<ExecResult, ProviderError> {
        let hash = command.starts_with("sha256sum ");
        let combined = command.contains("sha256sum -c -");
        if combined {
            assert!(
                command.find("sha256sum -c -").unwrap() < command.find("sudo -n install").unwrap()
            );
            assert!(!command.contains("private-boot-credential"));
        }
        self.calls.lock().unwrap().push(if hash {
            "read-artifact-hashes"
        } else {
            "launch"
        });
        Ok(ExecResult {
            exit_code: if combined && self.bad_bytes { 78 } else { 0 },
            stderr: vec![],
            stdout: if hash {
                format!("{}  /usr/local/bin/zuse-cloud-runtime\n{}  /opt/zuse/preparation/status/manifest.json\n",if self.bad_bytes{"c".repeat(64)}else{"a".repeat(64)},"b".repeat(64)).into_bytes()
            } else {
                vec![]
            },
        })
    }
    async fn upload(&self, _: &str, _: &str, bytes: &[u8]) -> Result<(), ProviderError> {
        assert_eq!(bytes, b"private-boot-credential");
        self.calls.lock().unwrap().push("deliver-boot-credential");
        Ok(())
    }
}
#[tokio::test]
async fn bound_artifact_is_verified_before_any_boot_credential_is_delivered() {
    for (bad_version, bad_bytes) in [(false, false), (true, false), (false, true)] {
        let fixture = Arc::new(BoundArtifactFixture {
            calls: Mutex::new(vec![]),
            bad_version,
            bad_bytes,
        });
        let mut provider = RuntimeProvider::new(fixture.clone(), "default".into());
        provider.artifact_binding = Some(crate::providers::artifact::ArtifactBinding {
            version: 1,
            mode: "private-single-build".into(),
            artifact_id: "template-one".into(),
            artifact_version: "build-one".into(),
            artifact_name: "zuse-e2b-stage-one".into(),
            runtime_sha256: "a".repeat(64),
            manifest_sha256: "b".repeat(64),
        });
        let result = provider
            .launch(
                "original",
                &RuntimeLaunch {
                    operation_id: "operation-one".into(),
                    workspace_id: "workspace-one".into(),
                    generation: 1,
                    incarnation: "disk-one".into(),
                    api_url: "https://cloud.invalid".into(),
                    boot_token: zeroize::Zeroizing::new("private-boot-credential".into()),
                },
            )
            .await;
        let observations = provider.startup_observations();
        assert!(observations["launchArtifactVerification"]["lastDurationMs"].is_u64());
        assert_eq!(
            observations.get("bootCredentialUpload").is_some(),
            !bad_version && !bad_bytes
        );
        assert_eq!(
            observations.get("runtimeLaunchCommand").is_some(),
            !bad_version && !bad_bytes
        );
        assert!(!observations.to_string().contains("private-boot-credential"));
        let calls = fixture.calls.lock().unwrap();
        if bad_version {
            assert_eq!(result.unwrap_err(), ProviderError::Rejected);
            assert_eq!(*calls, vec!["verify-version"])
        } else if bad_bytes {
            assert_eq!(result.unwrap_err(), ProviderError::Rejected);
            assert_eq!(*calls, vec!["verify-version", "read-artifact-hashes"])
        } else {
            assert!(result.is_ok());
            assert_eq!(
                *calls,
                vec![
                    "verify-version",
                    "read-artifact-hashes",
                    "deliver-boot-credential",
                    "launch"
                ]
            )
        }
    }
}

mod cadence;

#[tokio::test]
async fn prepared_launch_checks_bytes_and_starts_in_one_guest_call() {
    for bad_bytes in [false, true] {
        let fixture = Arc::new(BoundArtifactFixture {
            calls: Mutex::new(vec![]),
            bad_version: false,
            bad_bytes,
        });
        let mut provider = RuntimeProvider::new(fixture.clone(), "default".into());
        provider.fast_start = true;
        provider.artifact_binding = Some(crate::providers::artifact::ArtifactBinding {
            version: 1,
            mode: "private-single-build".into(),
            artifact_id: "template-one".into(),
            artifact_version: "build-one".into(),
            artifact_name: "zuse-e2b-stage-one".into(),
            runtime_sha256: "a".repeat(64),
            manifest_sha256: "b".repeat(64),
        });
        let result = provider
            .launch(
                "original",
                &RuntimeLaunch {
                    operation_id: "operation-one".into(),
                    workspace_id: "workspace-one".into(),
                    generation: 1,
                    incarnation: "disk-one".into(),
                    api_url: "https://cloud.invalid".into(),
                    boot_token: zeroize::Zeroizing::new("private-boot-credential".into()),
                },
            )
            .await;
        assert_eq!(result.is_ok(), !bad_bytes);
        assert_eq!(
            *fixture.calls.lock().unwrap(),
            vec!["verify-version", "deliver-boot-credential", "launch"]
        );
        assert!(
            provider
                .startup_observations()
                .get("launchBinaryVerification")
                .is_none()
        );
    }
}
