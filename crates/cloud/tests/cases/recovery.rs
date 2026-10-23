// Real guest, HTTP archive capabilities and PostgreSQL control-plane acceptance.
use std::sync::atomic::AtomicBool;
use zuse_cloud::objects::Objects;
use zuse_cloud::providers::{Machine, MachineState, ProviderError};
use zuse_cloud::runtime::recovery::{CapturePlan, RecoveryProvider};
struct ArchiveObjects {
    inner: Arc<ProjectionObjects>,
    corrupt: AtomicBool,
    block: AtomicBool,
    entered: Notify,
    release: Notify,
}
#[async_trait]
impl Objects for ArchiveObjects {
    fn supports_purge(&self) -> bool {
        self.inner.supports_purge()
    }
    async fn purge_prefix(&self, prefix: &str) -> Result<bool, ApiError> {
        self.inner.purge_prefix(prefix).await
    }
    async fn get(&self, key: &str) -> Result<Option<Vec<u8>>, ApiError> {
        self.inner.get(key).await
    }
    async fn put(&self, key: &str, bytes: &[u8]) -> Result<(), ApiError> {
        self.inner.put(key, bytes).await
    }
    async fn get_archive(&self, key: &str) -> Result<Option<Vec<u8>>, ApiError> {
        if !key.ends_with("/manifest") {
            if self.block.swap(false, Ordering::SeqCst) {
                self.entered.notify_one();
                self.release.notified().await;
            }
        }
        let mut bytes = self.inner.get(key).await?;
        if !key.ends_with("/manifest") && self.corrupt.load(Ordering::SeqCst) {
            if let Some(bytes) = &mut bytes {
                bytes[20] ^= 1;
            }
        }
        Ok(bytes)
    }
}
struct RecoveryMachine {
    state: std::sync::Mutex<MachineState>,
    destroyed: AtomicBool,
    lose_delete_reply: AtomicBool,
    retired: std::sync::Mutex<Vec<String>>,
    wakes: AtomicUsize,
    pool: PgPool,
    source: std::path::PathBuf,
    data: std::path::PathBuf,
    home: std::path::PathBuf,
    repository: std::path::PathBuf,
    helper: tokio::sync::Mutex<Option<tokio::process::Child>>,
    replacement: std::sync::Mutex<Option<Machine>>,
    creates: AtomicUsize,
    uncertain_create: AtomicBool,
    uncertain_restore: AtomicBool,
    uncertain_launch: AtomicBool,
    runtime: tokio::sync::Mutex<Option<tokio::process::Child>>,
}
#[async_trait]
impl RecoveryProvider for RecoveryMachine {
    async fn inspect(&self, id: &str) -> Result<Option<Machine>, ProviderError> {
        assert_eq!(id, "machine-bob");
        if self.destroyed.load(Ordering::SeqCst) {
            return Ok(None);
        }
        Ok(Some(Machine {
            id: id.into(),
            name: "original-machine".into(),
            state: self.state.lock().unwrap().clone(),
            vcpu: Some(2),
            memory_bytes: Some(1024 * 1024 * 1024),
        }))
    }
    async fn wake(&self, id: &str, _: &MachineState) -> Result<(), ProviderError> {
        assert_eq!(id, "machine-bob");
        self.wakes.fetch_add(1, Ordering::SeqCst);
        *self.state.lock().unwrap() = MachineState::Running;
        Ok(())
    }
    async fn capture(&self, plan: &CapturePlan) -> Result<(), ProviderError> {
        assert_eq!(plan.source_machine_id, "machine-bob");
        assert_eq!(plan.workspace_id, "ws-bob");
        let mut helper = self.helper.lock().await;
        if let Some(child) = helper.as_mut() {
            if child.try_wait().unwrap().is_none() {
                return Ok(());
            }
        }
        let mut config: Value = serde_json::from_str(&plan.config).unwrap();
        config["home"] = json!(self.home);
        config["repositoriesPath"] = json!(self.repository);
        let child =
            tokio::process::Command::new(std::env::var_os("ZUSE_CLOUD_TEST_GUEST_BIN").unwrap())
                .arg("--data-dir")
                .arg(&self.data)
                .arg("recover")
                .env("ZUSE_ARCHIVE_CONFIG", config.to_string())
                .env_remove("ZUSE_WORKSPACE_RUNTIME_LOCK_FD")
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .kill_on_drop(true)
                .spawn()
                .unwrap();
        *helper = Some(child);
        Ok(())
    }
    async fn stop_capture(&self, id: &str, _: &str) -> Result<(), ProviderError> {
        assert!(["machine-bob", "replacement-bob"].contains(&id));
        if let Some(mut child) = self.helper.lock().await.take() {
            if child.try_wait().unwrap().is_none() {
                child.start_kill().unwrap();
                let _ = child.wait().await;
            }
        }
        Ok(())
    }
    async fn pause(&self, id: &str) -> Result<(), ProviderError> {
        assert_eq!(id, "machine-bob");
        *self.state.lock().unwrap() = MachineState::Paused;
        Ok(())
    }
    async fn retire(&self, id: &str) -> Result<(), ProviderError> {
        assert_eq!(id, "machine-bob");
        let row:Value=sqlx::query_scalar("SELECT to_jsonb(o) FROM zuse_rust_recovery_operations o WHERE workspace_id='ws-bob' AND phase='retiring'").fetch_one(&self.pool).await.unwrap();
        assert!(row["verified_at"].is_number());
        assert_eq!(
            row["verified_archive_sha256"],
            row["manifest"]["archiveSha256"]
        );
        self.retired.lock().unwrap().push(id.into());
        self.destroyed.store(true, Ordering::SeqCst);
        std::fs::rename(&self.source, self.source.with_extension("retired")).unwrap();
        if self.lose_delete_reply.swap(false, Ordering::SeqCst) {
            return Err(ProviderError::OutcomeUnknown);
        }
        Ok(())
    }
}
#[async_trait]
impl zuse_cloud::runtime::allocation::AllocationProvider for RecoveryMachine {
    async fn find(&self, name: &str) -> Result<Option<Machine>, ProviderError> {
        Ok(self
            .replacement
            .lock()
            .unwrap()
            .as_ref()
            .filter(|m| m.id == name || m.name == name)
            .cloned())
    }
    async fn create(&self, snapshot: &str, name: &str) -> Result<Machine, ProviderError> {
        assert_eq!(snapshot, "approved-rust-snapshot");
        assert!(self.destroyed.load(Ordering::SeqCst));
        assert!(self.replacement.lock().unwrap().is_none());
        self.creates.fetch_add(1, Ordering::SeqCst);
        for path in [&self.data, &self.home, &self.repository] {
            std::fs::create_dir_all(path).unwrap();
        }
        let machine = Machine {
            id: "replacement-bob".into(),
            name: name.into(),
            state: MachineState::Running,
            vcpu: Some(2),
            memory_bytes: Some(1024 * 1024 * 1024),
        };
        *self.replacement.lock().unwrap() = Some(machine.clone());
        if self.uncertain_create.swap(false, Ordering::SeqCst) {
            return Err(ProviderError::OutcomeUnknown);
        }
        Ok(machine)
    }
    async fn launch(
        &self,
        id: &str,
        launch: &zuse_cloud::runtime::allocation::RuntimeLaunch,
    ) -> Result<(), ProviderError> {
        assert_eq!(id, "replacement-bob");
        assert_eq!(launch.generation, 3);
        assert_eq!(launch.incarnation, "packaged-storage");
        let mut runtime = self.runtime.lock().await;
        if runtime
            .as_mut()
            .is_some_and(|p| p.try_wait().unwrap().is_none())
        {
            return Ok(());
        }
        let boot = self.source.join("fresh-boot");
        std::fs::write(&boot, launch.boot_token.as_bytes()).unwrap();
        let child =
            tokio::process::Command::new(std::env::var_os("ZUSE_CLOUD_TEST_GUEST_BIN").unwrap())
                .args([
                    "--workspace-id",
                    &launch.workspace_id,
                    "--api-url",
                    &launch.api_url,
                    "--generation",
                    &launch.generation.to_string(),
                    "--storage-incarnation-id",
                    &launch.incarnation,
                    "--ipc-port",
                    "0",
                    "--test-harness",
                ])
                .arg("--data-dir")
                .arg(&self.data)
                .arg("--workspace-root")
                .arg(&self.repository)
                .arg("--boot-token-file")
                .arg(boot)
                .env_remove("ZUSE_WORKSPACE_RUNTIME_LOCK_FD")
                .env_remove("ZUSE_RUNTIME_BOOT_TOKEN")
                .env_remove("ZUSE_RUNTIME_BOOT_TOKEN_FILE")
                .stdout(std::process::Stdio::null())
                .stderr(std::fs::File::create(self.source.join("replacement.log")).unwrap())
                .kill_on_drop(true)
                .spawn()
                .unwrap();
        *runtime = Some(child);
        if self.uncertain_launch.swap(false, Ordering::SeqCst) {
            return Err(ProviderError::OutcomeUnknown);
        }
        Ok(())
    }
}
#[async_trait]
impl zuse_cloud::runtime::restore::RestoreProvider for RecoveryMachine {
    async fn restore(&self, id: &str, plan: &CapturePlan) -> Result<(), ProviderError> {
        assert_eq!(id, "replacement-bob");
        // The physical targets have the same guest paths, as real replacement VMs do.
        RecoveryProvider::capture(self, plan).await?;
        if self.uncertain_restore.swap(false, Ordering::SeqCst) {
            return Err(ProviderError::OutcomeUnknown);
        }
        Ok(())
    }
    async fn installed(&self, id: &str, _: &str) -> Result<bool, ProviderError> {
        assert_eq!(id, "replacement-bob");
        let mut helper = self.helper.lock().await;
        if let Some(child) = helper.as_mut() {
            if let Some(status) = child.try_wait().unwrap() {
                assert!(status.success(), "Restore helper failed");
                return Ok(self
                    .data
                    .join(zuse_cloud_runtime::recovery::RESTORE_RECEIPT)
                    .is_file());
            }
        }
        Ok(false)
    }
}
struct ArchiveFixture {
    api: GuestApi,
    control: Arc<zuse_cloud::runtime::RuntimeControl>,
    objects: Arc<ArchiveObjects>,
    machine: RecoveryMachine,
    _temp: tempfile::TempDir,
}
impl ArchiveFixture {
    async fn start() -> Self {
        use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD as B64};
        use ed25519_dalek::SigningKey;
        let api = GuestApi::start_with_projections().await;
        api.quiesced_lost_ack.store(true, Ordering::SeqCst);
        sqlx::raw_sql(include_str!("../fixtures/creation.sql"))
            .execute(&api.pool)
            .await
            .unwrap();
        let now = chrono::Utc::now().timestamp_millis();
        sqlx::query("INSERT INTO api_cloud_billing_periods(period_id,account_id,status,period_start,period_end,included_provider_cost_micros,markup_basis_points,overage_cap_micros) VALUES('recovery-period','user-bob','active',$1,$2,100000,500,500)").bind(now-1000).bind(now+86400000).execute(&api.pool).await.unwrap();
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let data = source.join("data");
        let home = source.join("home");
        let checkout = source.join("repository");
        std::fs::create_dir_all(&checkout).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        std::fs::write(checkout.join(".env"), "SAVED_USER_ENV=yes").unwrap();
        let boot = temp.path().join("boot");
        std::fs::write(&boot, "packaged-guest-boot").unwrap();
        let log = temp.path().join("guest.log");
        let mut guest = guest_command(&api, &data, &checkout, Some(&boot), &log)
            .spawn()
            .unwrap();
        tokio::time::timeout(Duration::from_secs(20),async{loop{
            if let Some(status)=guest.try_wait().unwrap(){panic!("Guest failed {status}: {}",std::fs::read_to_string(&log).unwrap());}
            let complete:bool=sqlx::query_scalar("SELECT NOT EXISTS(SELECT 1 FROM api_cloud_workspace_launch_intents WHERE workspace_id='ws-bob')").fetch_one(&api.pool).await.unwrap();if complete{break}tokio::time::sleep(Duration::from_millis(20)).await;
        }}).await.unwrap();
        let status = runtime_post_for_url(
            &format!("{}/v1/cloud/workspaces/ws-bob/archive", api.origin),
            "bob",
            &json!({"workspaceId":"ws-bob","commandId":"archive-recovery-source"}),
        )
        .await;
        assert_eq!(status.0, 200, "{}", status.1);
        let status = tokio::time::timeout(Duration::from_secs(20), guest.wait())
            .await
            .unwrap_or_else(|e| {
                panic!(
                    "Archive shutdown timed out: {e}: {}",
                    std::fs::read_to_string(&log).unwrap()
                )
            })
            .unwrap();
        assert!(
            status.success(),
            "Archive guest failed {status}: {}",
            std::fs::read_to_string(&log).unwrap()
        );
        let retained = RetainedFixture {
            state: std::sync::Mutex::new(MachineState::Running),
            pauses: AtomicUsize::new(0),
            wakes: AtomicUsize::new(0),
            launches: AtomicUsize::new(0),
            fail_pause: AtomicBool::new(false),
        };
        for _ in 0..2 {
            sqlx::query("UPDATE zuse_rust_jobs SET due_at=0 WHERE kind='workspace-lifecycle'")
                .execute(&api.pool)
                .await
                .unwrap();
            api.control
                .lifecycle_once(&retained, chrono::Utc::now().timestamp_millis())
                .await
                .unwrap();
        }
        sqlx::query("UPDATE api_cloud_workspaces SET archive_requested_at=$1,archive_delete_at=$2,request_config=jsonb_set(request_config,'{providerAuthMode}','\"broker-v1\"') WHERE workspace_id='ws-bob'").bind(now-zuse_protocol::cloud::ARCHIVE_RETENTION_MS-1000).bind(now-1000).execute(&api.pool).await.unwrap();
        let objects = Arc::new(ArchiveObjects {
            inner: api.projection_objects.as_ref().unwrap().clone(),
            corrupt: AtomicBool::new(false),
            block: AtomicBool::new(false),
            entered: Notify::new(),
            release: Notify::new(),
        });
        let mint = SigningKey::from_bytes(&[14; 32]);
        let jwk = json!({"kty":"OKP","crv":"Ed25519","d":B64.encode([14;32]),"x":B64.encode(mint.verifying_key().as_bytes())});
        let control = Arc::new(
            zuse_cloud::runtime::RuntimeControl::from_pool(
                api.pool.clone(),
                zuse_cloud::crypto::MasterKey::parse(&B64.encode([15; 32])).unwrap(),
                zuse_cloud::tickets::TicketSigner::from_jwk(&jwk.to_string(), &api.origin).unwrap(),
            )
            .with_transcript_objects(objects.clone())
            .with_recovery_policy(true, vec!["ws-bob".into()]),
        );
        let machine = RecoveryMachine {
            state: std::sync::Mutex::new(MachineState::Paused),
            destroyed: AtomicBool::new(false),
            lose_delete_reply: AtomicBool::new(false),
            retired: std::sync::Mutex::new(vec![]),
            wakes: AtomicUsize::new(0),
            pool: api.pool.clone(),
            source,
            data,
            home,
            repository: checkout,
            helper: tokio::sync::Mutex::new(None),
            replacement: std::sync::Mutex::new(None),
            creates: AtomicUsize::new(0),
            uncertain_create: AtomicBool::new(false),
            uncertain_restore: AtomicBool::new(false),
            uncertain_launch: AtomicBool::new(false),
            runtime: tokio::sync::Mutex::new(None),
        };
        Self {
            api,
            control,
            objects,
            machine,
            _temp: temp,
        }
    }
    async fn tick(&self) -> Result<bool, ApiError> {
        sqlx::query("UPDATE zuse_rust_jobs SET due_at=0 WHERE kind='workspace-recovery'")
            .execute(&self.api.pool)
            .await
            .unwrap();
        self.control
            .recovery_once(&self.machine, chrono::Utc::now().timestamp_millis())
            .await
    }
    async fn phase(&self) -> String {
        sqlx::query_scalar("SELECT phase FROM zuse_rust_recovery_operations WHERE workspace_id='ws-bob' ORDER BY created_at DESC LIMIT 1").fetch_one(&self.api.pool).await.unwrap()
    }
    async fn upload(&self) {
        self.tick().await.unwrap();
        self.tick().await.unwrap();
        tokio::time::timeout(Duration::from_secs(20), async {
            while self.phase().await != "verifying" {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
        self.tick().await.unwrap();
        assert_eq!(*self.machine.state.lock().unwrap(), MachineState::Paused);
        assert_eq!(self.phase().await, "verifying");
    }
    async fn unarchive(&self, id: &str) -> Value {
        let (status, value) = runtime_post_for_url(
            &format!("{}/v1/cloud/workspaces/ws-bob/unarchive", self.api.origin),
            "bob",
            &json!({"workspaceId":"ws-bob","commandId":id}),
        )
        .await;
        assert_eq!(status, 200, "{value}");
        value
    }
    async fn finish(self) {
        if let Some(mut child) = self.machine.helper.lock().await.take() {
            if child.try_wait().unwrap().is_none() {
                child.start_kill().unwrap();
                let _ = child.wait().await;
            }
        }
        if let Some(mut runtime) = self.machine.runtime.lock().await.take() {
            stop_guest(&mut runtime).await;
        }
        self.api.finish().await;
    }
}

#[tokio::test]
#[ignore = "requires isolated PostgreSQL and packaged guest"]
async fn full_backup_verification_barrier_precedes_exact_source_retirement_and_lost_reply_recovery()
{
    use rand::RngCore;
    let f = ArchiveFixture::start().await;
    // Flag-off expiry cannot wake or delete even a supported native guest.
    assert!(
        f.api
            .control
            .begin_recovery("ws-bob", chrono::Utc::now().timestamp_millis())
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(f.machine.wakes.load(Ordering::SeqCst), 0);
    let mut bytes = vec![0; 4 * 1024 * 1024 + 100000];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    std::fs::write(f.machine.repository.join("untracked-large.bin"), &bytes).unwrap();
    let plan = f
        .control
        .begin_recovery("ws-bob", chrono::Utc::now().timestamp_millis())
        .await
        .unwrap()
        .unwrap();
    let config: Value = serde_json::from_str(&plan.config).unwrap();
    let url = config["url"].as_str().unwrap();
    let token = config["token"].as_str().unwrap();
    assert_eq!(
        reqwest::Client::new()
            .get(format!("{url}/manifest"))
            .bearer_auth(token)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        reqwest::Client::new()
            .put(format!("{url}/{}", "0".repeat(64)))
            .bearer_auth(token)
            .body("invalid chunk")
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::BAD_REQUEST
    );
    f.upload().await;
    assert!(f.machine.retired.lock().unwrap().is_empty());
    f.tick().await.unwrap();
    assert_eq!(f.phase().await, "retiring");
    assert!(f.machine.retired.lock().unwrap().is_empty());
    // Public recovery metadata cannot redirect retirement away from the immutable source pin.
    sqlx::query("UPDATE api_cloud_workspaces SET request_config=jsonb_set(request_config,'{recoveryArchive,sourceSandboxId}','\"other-machine\"') WHERE workspace_id='ws-bob'").execute(&f.api.pool).await.unwrap();
    assert_eq!(
        f.tick().await.unwrap_err().code,
        "cloud_archive_operation_fenced"
    );
    assert!(f.machine.retired.lock().unwrap().is_empty());
    sqlx::query("UPDATE api_cloud_workspaces SET request_config=jsonb_set(request_config,'{recoveryArchive,sourceSandboxId}','\"machine-bob\"') WHERE workspace_id='ws-bob'").execute(&f.api.pool).await.unwrap();
    // Recheck sharing immediately before destruction, including references added after admission.
    sqlx::query("UPDATE api_cloud_workspaces SET provider_sandbox_id='machine-bob' WHERE workspace_id='ws-alice'").execute(&f.api.pool).await.unwrap();
    assert_eq!(f.tick().await.unwrap_err().code, "archive-source-shared");
    assert!(f.machine.retired.lock().unwrap().is_empty());
    sqlx::query("UPDATE api_cloud_workspaces SET provider_sandbox_id='machine-alice' WHERE workspace_id='ws-alice'").execute(&f.api.pool).await.unwrap();
    let before = f.unarchive("unarchive-after-barrier").await;
    assert_eq!(before["state"], "paused");
    f.machine.lose_delete_reply.store(true, Ordering::SeqCst);
    assert_eq!(
        f.tick().await.unwrap_err().code,
        "archive-retirement-failed"
    );
    assert_eq!(f.phase().await, "retiring");
    f.tick().await.unwrap();
    assert_eq!(f.phase().await, "retired");
    assert_eq!(&*f.machine.retired.lock().unwrap(), &["machine-bob"]);
    let row: Value = sqlx::query_scalar(
        "SELECT to_jsonb(w) FROM api_cloud_workspaces w WHERE workspace_id='ws-bob'",
    )
    .fetch_one(&f.api.pool)
    .await
    .unwrap();
    assert!(row["provider_sandbox_id"].is_null());
    assert_eq!(row["state"], "paused");
    let status = zuse_cloud::archive::status(&row);
    assert!(status.backup_verified && status.compute_retired && !status.machine_retained);
    let history = zuse_cloud_projection::reader::Reader::new(&f.api.origin)
        .unwrap()
        .head("bob", "ws-bob", "session-bob")
        .await
        .unwrap()
        .unwrap();
    assert!(
        serde_json::to_string(&history.transcript)
            .unwrap()
            .contains("Cloud runtime fixture reply")
    );
    let events: Vec<Value> = sqlx::query_scalar("SELECT payload FROM api_cloud_usage_outbox")
        .fetch_all(&f.api.pool)
        .await
        .unwrap();
    assert!(!events.is_empty());
    assert!(events.iter().all(|e| e["metadata"]["billable"] == "false"
        && e["accountId"] == "user-bob"
        && e["metadata"]["provider_sandbox_id"] == "machine-bob"));
    f.finish().await;
}
#[tokio::test]
#[ignore = "requires isolated PostgreSQL and packaged guest"]
async fn corrupt_backup_retains_source_and_unarchive_cancels_before_the_barrier() {
    let f = ArchiveFixture::start().await;
    f.control
        .begin_recovery("ws-bob", chrono::Utc::now().timestamp_millis())
        .await
        .unwrap()
        .unwrap();
    f.upload().await;
    f.objects.corrupt.store(true, Ordering::SeqCst);
    assert_eq!(
        f.tick().await.unwrap_err().code,
        "archive-verification-failed"
    );
    assert_eq!(f.phase().await, "verifying");
    assert!(f.machine.retired.lock().unwrap().is_empty());
    assert!(f.machine.source.exists());
    let value = f.unarchive("cancel-corrupt-backup").await;
    assert_eq!(value["state"], "paused");
    assert_eq!(f.phase().await, "cancelled");
    f.tick().await.unwrap();
    let row: Value = sqlx::query_scalar(
        "SELECT to_jsonb(w) FROM api_cloud_workspaces w WHERE workspace_id='ws-bob'",
    )
    .fetch_one(&f.api.pool)
    .await
    .unwrap();
    assert!(row["runtime_credential_hash"].is_string());
    assert_eq!(row["provider_sandbox_id"], "machine-bob");
    assert!(row["request_config"]["rustRecoveryCancellation"].is_null());
    assert!(f.machine.retired.lock().unwrap().is_empty());
    f.finish().await;
}
#[tokio::test]
#[ignore = "requires isolated PostgreSQL and packaged guest"]
async fn unarchive_and_delete_fence_an_inflight_verifier() {
    let f = ArchiveFixture::start().await;
    f.control
        .begin_recovery("ws-bob", chrono::Utc::now().timestamp_millis())
        .await
        .unwrap()
        .unwrap();
    f.upload().await;
    f.objects.block.store(true, Ordering::SeqCst);
    let current = f.control.clone();
    let pool = f.api.pool.clone();
    sqlx::query("UPDATE zuse_rust_jobs SET due_at=0 WHERE kind='workspace-recovery'")
        .execute(&pool)
        .await
        .unwrap();
    {
        let verify = current.recovery_once(&f.machine, chrono::Utc::now().timestamp_millis());
        tokio::pin!(verify);
        tokio::select! {result=&mut verify=>panic!("Verifier completed before its read: {result:?}"),_=f.objects.entered.notified()=>{}}
        let value = f.unarchive("cancel-inflight-verifier").await;
        assert_eq!(value["state"], "paused");
        f.objects.release.notify_one();
        assert_eq!(
            verify.await.unwrap_err().code,
            "cloud_archive_operation_fenced"
        );
    }
    assert_eq!(f.phase().await, "cancelled");
    assert!(f.machine.retired.lock().unwrap().is_empty());
    // A deleted workspace cannot grant archive bytes or be resurrected by its delayed cleanup job.
    sqlx::query("UPDATE api_cloud_workspaces SET desired_state='deleted',state='deleting' WHERE workspace_id='ws-bob'").execute(&f.api.pool).await.unwrap();
    assert!(f.tick().await.is_err());
    assert!(f.machine.retired.lock().unwrap().is_empty());
    f.finish().await;
}

#[tokio::test]
#[ignore = "requires isolated PostgreSQL and packaged guest"]
async fn retired_archive_restores_into_fresh_generation_and_continues_original_conversation() {
    use zuse_cloud_runtime::mailbox::CommandKey;
    for request_during_retirement in [false, true] {
        let f = ArchiveFixture::start().await;
        sqlx::query(
        "UPDATE api_cloud_workspaces SET project_id='create-project' WHERE workspace_id='ws-bob'",
    )
    .execute(&f.api.pool)
    .await
    .unwrap();
        f.control
            .begin_recovery("ws-bob", chrono::Utc::now().timestamp_millis())
            .await
            .unwrap()
            .unwrap();
        f.upload().await;
        f.tick().await.unwrap();
        if !request_during_retirement {
            f.tick().await.unwrap();
            assert_eq!(f.phase().await, "retired");
        } else {
            assert_eq!(f.phase().await, "retiring");
        }
        assert_eq!(f.machine.creates.load(Ordering::SeqCst), 0);
        f.unarchive("unarchive-for-restore").await;
        assert_eq!(f.machine.creates.load(Ordering::SeqCst), 0);
        let (status, value) = runtime_post_for_url(
            &format!("{}/v1/cloud/workspaces/ws-bob/resume", f.api.origin),
            "bob",
            &json!({"workspaceId":"ws-bob","commandId":"resume-original-archive"}),
        )
        .await;
        assert_eq!(status, 200, "{value}");
        if request_during_retirement {
            assert_eq!(f.phase().await, "retiring");
            assert!(f.machine.source.is_dir());
            assert_eq!(f.machine.creates.load(Ordering::SeqCst), 0);
            let unchanged = f.unarchive("repeat-unarchive-after-demand").await;
            assert_eq!(unchanged["desiredState"], "ready");
            f.tick().await.unwrap();
            assert_eq!(f.phase().await, "retired");
            assert_eq!(f.machine.creates.load(Ordering::SeqCst), 0);
            f.tick().await.unwrap();
        }
        assert_eq!(f.phase().await, "restoring");
        let unchanged = f.unarchive("repeat-unarchive-during-restore").await;
        assert_eq!(unchanged["desiredState"], "ready");
        f.machine.uncertain_create.store(true, Ordering::SeqCst);
        f.machine.uncertain_restore.store(true, Ordering::SeqCst);
        f.machine.uncertain_launch.store(true, Ordering::SeqCst);
        let mut uncertain = 0;
        let mut expired_before_bootstrap = false;
        tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                sqlx::query("UPDATE zuse_rust_jobs SET due_at=0 WHERE kind='workspace-restore'")
                    .execute(&f.api.pool)
                    .await
                    .unwrap();
                if let Err(error) = f
                    .control
                    .restore_once(&f.machine, chrono::Utc::now().timestamp_millis())
                    .await
                {
                    assert_eq!(error.code, "cloud_archive_restore_failed");
                    uncertain += 1;
                    if uncertain==1 {
                        // Resume retries a failed delivery against the same persisted replacement.
                        sqlx::query("UPDATE zuse_rust_jobs SET state='failed' WHERE kind='workspace-restore'").execute(&f.api.pool).await.unwrap();
                        let (status,value)=runtime_post_for_url(&format!("{}/v1/cloud/workspaces/ws-bob/resume",f.api.origin),"bob",&json!({"workspaceId":"ws-bob","commandId":"resume-original-archive"})).await;
                        assert_eq!(status,200,"{value}");
                        let state:String=sqlx::query_scalar("SELECT state FROM zuse_rust_jobs WHERE kind='workspace-restore'").fetch_one(&f.api.pool).await.unwrap();
                        assert_eq!(state,"pending");
                        // Allocation and download can outlast the original unpublished boot window.
                        sqlx::query("UPDATE zuse_rust_restores SET boot_expires_at=0").execute(&f.api.pool).await.unwrap();
                    }
                    assert!(f.machine.source.is_dir());
                    assert!(
                        f.objects
                            .inner
                            .bytes
                            .lock()
                            .unwrap()
                            .keys()
                            .any(|k| k.contains("/archives/") && k.ends_with("manifest"))
                    );
                }
                if !expired_before_bootstrap {
                    let phase:String=sqlx::query_scalar("SELECT phase FROM zuse_rust_restores").fetch_one(&f.api.pool).await.unwrap();
                    if phase=="restored" {
                        sqlx::query("UPDATE zuse_rust_restores SET boot_expires_at=0").execute(&f.api.pool).await.unwrap();
                        sqlx::query("UPDATE api_cloud_workspaces SET runtime_boot_token_expires_at=0 WHERE workspace_id='ws-bob'").execute(&f.api.pool).await.unwrap();
                        expired_before_bootstrap=true;
                    }
                }
                if let Some(child) = f.machine.runtime.lock().await.as_mut() {
                    if let Some(status) = child.try_wait().unwrap() {
                        panic!(
                            "Restored runtime failed {status}: {}",
                            std::fs::read_to_string(f.machine.source.join("replacement.log"))
                                .unwrap()
                        );
                    }
                }
                if f.phase().await == "restored" {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(f.machine.creates.load(Ordering::SeqCst), 1);
        assert_eq!(uncertain, 3);
        assert!(expired_before_bootstrap);
        assert_eq!(
            std::fs::read_to_string(f.machine.repository.join(".env")).unwrap(),
            "SAVED_USER_ENV=yes"
        );
        let row: Value = sqlx::query_scalar(
            "SELECT to_jsonb(w) FROM api_cloud_workspaces w WHERE workspace_id='ws-bob'",
        )
        .fetch_one(&f.api.pool)
        .await
        .unwrap();
        assert_eq!(row["provider_sandbox_id"], "replacement-bob");
        assert_eq!(row["request_config"]["runtimeGeneration"], 3);
        assert_eq!(
            row["request_config"]["rustRuntimeStorage"]["storageIncarnationId"],
            "packaged-storage"
        );
        assert_eq!(row["chat_id"], "chat-bob");
        assert_eq!(row["initial_session_id"], "session-bob");
        let key_response: Value = reqwest::Client::new()
            .get(format!(
                "{}/v1/cloud/workspaces/ws-bob/data-key",
                f.api.origin
            ))
            .bearer_auth("bob")
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let key = CommandKey::parse(key_response["encodedKey"].as_str().unwrap()).unwrap();
        let envelope = encrypted_mailbox_message(
            &key,
            "restored-continuation",
            "Continue the original conversation after restoration",
        );
        assert_eq!(
            runtime_post_for_url(
                &format!("{}/v1/cloud/workspaces/ws-bob/commands", f.api.origin),
                "bob",
                &envelope
            )
            .await
            .0,
            202
        );
        wait_command_state(&f.api, "restored-continuation", "applied").await;
        let client = f.api.client().await;
        let mut messages = client
            .client
            .subscribe(
                zuse_rpc::methods::WATCH_DOC_MESSAGES,
                json!({"chatId":"session-bob"}),
            )
            .await
            .unwrap();
        let mut last = String::new();
        let response = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let value = messages.recv().await.unwrap();
                last = value.to_string();
                let reset_complete = value["reset"].as_array().is_some_and(|entries| {
                    entries
                        .iter()
                        .filter(|e| e["role"] == "assistant" && e["status"] == "complete")
                        .count()
                        >= 2
                });
                let new_complete = value["upsert"].as_array().is_some_and(|entries| {
                    entries.iter().any(|e| {
                        e["after"] == "message-restored-continuation"
                            && e["entry"]["role"] == "assistant"
                            && e["entry"]["status"] == "complete"
                    })
                });
                if reset_complete || new_complete {
                    break;
                }
            }
        })
        .await;
        assert!(
            response.is_ok(),
            "Restored response missing: {last}; guest: {}",
            std::fs::read_to_string(f.machine.source.join("replacement.log")).unwrap()
        );
        // The live protocol sends deltas. Reopen the watch to inspect the authoritative full reset.
        drop(messages);
        let mut messages = client
            .client
            .subscribe(
                zuse_rpc::methods::WATCH_DOC_MESSAGES,
                json!({"chatId":"session-bob"}),
            )
            .await
            .unwrap();
        let history = tokio::time::timeout(Duration::from_secs(5), messages.recv())
            .await
            .unwrap()
            .unwrap()
            .to_string();
        assert_eq!(history.matches("Original guest prompt").count(), 1);
        assert_eq!(history.matches("Cloud runtime fixture reply").count(), 2);
        assert_eq!(
            history
                .matches("Continue the original conversation after restoration")
                .count(),
            1
        );
        drop(messages);
        client.closed.cancel();
        drop(client);
        f.finish().await;
    }
}
