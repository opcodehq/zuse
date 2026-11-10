// Packaged guest and real local PostgreSQL. Provider suspension is simulated;
// this proves the retained guest handshake, not a live provider's RAM guarantee.
use zuse_cloud::providers::contract::{DesiredState, OperationState, ProviderOperation};
use zuse_cloud::providers::{Machine, MachineState, ProviderError};
use zuse_cloud::runtime::lifecycle::RetainedProvider;

struct MemoryFixture {
    inner: RetainedFixture,
    data: std::path::PathBuf,
}
struct TerminalCleanup(i32);
impl Drop for TerminalCleanup {
    fn drop(&mut self) {
        // This PID is read from the isolated fixture's newly created PTY.
        unsafe {
            libc::kill(-self.0, libc::SIGKILL);
        }
    }
}
#[async_trait]
impl RetainedProvider for MemoryFixture {
    async fn inspect_retained(&self, id: &str) -> Result<Option<Machine>, ProviderError> {
        self.inner.inspect_retained(id).await
    }
    async fn pause_retained(&self, _: &str) -> Result<(), ProviderError> {
        panic!("memory parking cannot silently use disk-only hibernation")
    }
    async fn memory_sleep_receipt(&self, id: &str) -> Result<ProviderOperation, ProviderError> {
        assert_eq!(id, "machine-bob");
        self.inner.pauses.fetch_add(1, Ordering::SeqCst);
        *self.inner.state.lock().unwrap() = MachineState::Standby;
        Ok(ProviderOperation {
            resource_id: id.into(),
            operation_id: Some("original-memory-suspend".into()),
            desired: DesiredState::Sleeping,
        })
    }
    async fn memory_receipt_state(
        &self,
        receipt: &ProviderOperation,
    ) -> Result<OperationState, ProviderError> {
        assert_eq!(
            receipt.operation_id.as_deref(),
            Some("original-memory-suspend")
        );
        assert_eq!(receipt.resource_id, "machine-bob");
        Ok(
            if *self.inner.state.lock().unwrap() == MachineState::Standby {
                OperationState::Completed
            } else {
                OperationState::Superseded
            },
        )
    }
    async fn wake_retained(&self, id: &str, state: &MachineState) -> Result<(), ProviderError> {
        self.inner.wake_retained(id, state).await
    }
    async fn launch_retained(
        &self,
        directive: &zuse_protocol::cloud_lifecycle::Directive,
        workspace: &str,
        _: &str,
    ) -> Result<(), ProviderError> {
        assert_eq!(workspace, "ws-bob");
        self.inner.launches.fetch_add(1, Ordering::SeqCst);
        let status =
            tokio::process::Command::new(std::env::var_os("ZUSE_CLOUD_TEST_GUEST_BIN").unwrap())
                .args([
                    "--workspace-id",
                    workspace,
                    "--generation",
                    "2",
                    "--storage-incarnation-id",
                    "packaged-storage",
                    "--data-dir",
                ])
                .arg(&self.data)
                .args(["notify-resume", "--operation-id", &directive.operation_id])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .await
                .map_err(|_| ProviderError::Unavailable)?;
        if status.success() {
            Ok(())
        } else {
            Err(ProviderError::Unavailable)
        }
    }
}
async fn memory_tick(api: &GuestApi, provider: &MemoryFixture) {
    sqlx::query("UPDATE zuse_rust_jobs SET due_at=0 WHERE kind='workspace-lifecycle' AND state NOT IN ('completed','cancelled')")
        .execute(&api.pool).await.unwrap();
    api.control
        .lifecycle_once(provider, chrono::Utc::now().timestamp_millis())
        .await
        .unwrap();
}
#[tokio::test]
#[ignore = "requires isolated localhost PostgreSQL and current packaged guest"]
async fn packaged_memory_resume_preserves_process_terminal_dirty_work_and_recovers_lost_park_ack() {
    let api = GuestApi::start_with_projections().await;
    sqlx::raw_sql(include_str!("../fixtures/creation.sql"))
        .execute(&api.pool)
        .await
        .unwrap();
    let now = chrono::Utc::now().timestamp_millis();
    sqlx::query("INSERT INTO api_cloud_billing_periods(period_id,account_id,status,period_start,period_end,included_provider_cost_micros,markup_basis_points,overage_cap_micros) VALUES('memory-period','user-bob','active',$1,$2,1000,500,500)")
        .bind(now-1000).bind(now+86_400_000).execute(&api.pool).await.unwrap();
    sqlx::query("UPDATE api_cloud_workspaces SET request_config=jsonb_set(request_config,'{rustMemorySleepVersion}','1') WHERE workspace_id='ws-bob'")
        .execute(&api.pool).await.unwrap();
    let temp = tempfile::tempdir().unwrap();
    let data = temp.path().join("runtime");
    let checkout = temp.path().join("checkout");
    std::fs::create_dir(&checkout).unwrap();
    std::fs::write(checkout.join("dirty.txt"), "retained uncommitted work").unwrap();
    let boot = temp.path().join("boot");
    std::fs::write(&boot, "packaged-guest-boot").unwrap();
    let log = temp.path().join("guest.log");
    let mut child = guest_command(&api, &data, &checkout, Some(&boot), &log)
        .spawn()
        .unwrap();
    let runtime_pid = child.id().unwrap();
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            if let Some(status) = child.try_wait().unwrap() { panic!("guest exited {status}: {}", std::fs::read_to_string(&log).unwrap()); }
            let done: bool = sqlx::query_scalar("SELECT NOT EXISTS(SELECT 1 FROM api_cloud_workspace_launch_intents WHERE workspace_id='ws-bob')").fetch_one(&api.pool).await.unwrap();
            if done { break; }
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
    }).await.unwrap();
    let client = api.client().await;
    let mut transcript = client
        .client
        .subscribe(
            zuse_rpc::methods::WATCH_DOC_MESSAGES,
            json!({"chatId":"session-bob"}),
        )
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        while !transcript
            .recv()
            .await
            .unwrap()
            .to_string()
            .contains("Cloud runtime fixture reply")
        {}
    })
    .await
    .unwrap();
    drop(transcript);
    // Let the mock's completed turn settle before the idle parking boundary.
    tokio::time::sleep(Duration::from_millis(150)).await;
    let terminal = client
        .client
        .call(
            zuse_rpc::methods::OPEN_TERMINAL,
            json!({"chatId":"session-bob","cols":80,"rows":24}),
        )
        .await
        .unwrap();
    let pid_file = checkout.join("terminal.pid");
    client.client.call(zuse_rpc::methods::WRITE_TERMINAL, json!({"terminalId":terminal["id"],"data":format!("echo $$ > '{}'; export MEMORY_RETAINED=yes\n",pid_file.display())})).await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while !pid_file.exists() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    let terminal_pid: i32 = std::fs::read_to_string(&pid_file)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    let _terminal_cleanup = TerminalCleanup(terminal_pid);
    api.quiesced_lost_ack.store(true, Ordering::SeqCst);
    let paused = runtime_post_for_url(
        &format!("{}/v1/cloud/workspaces/ws-bob/pause", api.origin),
        "bob",
        &json!({"workspaceId":"ws-bob","commandId":"memory-pause"}),
    )
    .await;
    assert_eq!(paused.0, 200, "{}", paused.1);
    let provider = MemoryFixture {
        inner: RetainedFixture {
            state: std::sync::Mutex::new(MachineState::Running),
            pauses: AtomicUsize::new(0),
            wakes: AtomicUsize::new(0),
            launches: AtomicUsize::new(0),
            fail_pause: std::sync::atomic::AtomicBool::new(false),
        },
        data: data.clone(),
    };
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            if let Some(status) = child.try_wait().unwrap() {
                panic!(
                    "park exited {status}: {}",
                    std::fs::read_to_string(&log).unwrap()
                );
            }
            memory_tick(&api, &provider).await;
            let state: String = sqlx::query_scalar(
                "SELECT state FROM api_cloud_workspaces WHERE workspace_id='ws-bob'",
            )
            .fetch_one(&api.pool)
            .await
            .unwrap();
            if state == "paused" {
                break;
            }
            tokio::time::sleep(Duration::from_millis(40)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(child.id(), Some(runtime_pid));
    assert_eq!(
        unsafe { libc::kill(terminal_pid, 0) },
        0,
        "park must retain the PTY process"
    );
    assert_eq!(provider.inner.pauses.load(Ordering::SeqCst), 1);
    client.closed.cancel();
    drop(client);
    // The bearer expires during a long sleep; retained recipient proof must
    // renew it after an explicit Resume without any initial prompt replay.
    sqlx::query("UPDATE api_cloud_workspaces SET request_config=jsonb_set(request_config,'{runtimeCredentialExpiresAtMs}',$1) WHERE workspace_id='ws-bob'").bind(json!(now-1)).execute(&api.pool).await.unwrap();
    let resume_started = std::time::Instant::now();
    let resumed = runtime_post_for_url(
        &format!("{}/v1/cloud/workspaces/ws-bob/resume", api.origin),
        "bob",
        &json!({"workspaceId":"ws-bob","commandId":"memory-resume"}),
    )
    .await;
    assert_eq!(resumed.0, 200, "{}", resumed.1);
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            if let Some(status) = child.try_wait().unwrap() {
                panic!(
                    "resume exited {status}: {}",
                    std::fs::read_to_string(&log).unwrap()
                );
            }
            memory_tick(&api, &provider).await;
            let state: String = sqlx::query_scalar(
                "SELECT state FROM api_cloud_workspaces WHERE workspace_id='ws-bob'",
            )
            .fetch_one(&api.pool)
            .await
            .unwrap();
            if state == "ready" {
                break;
            }
            tokio::time::sleep(Duration::from_millis(40)).await;
        }
    })
    .await
    .unwrap();
    eprintln!(
        "packaged memory fixture resume to server-ready: {} ms (simulated provider, expired bearer)",
        resume_started.elapsed().as_millis()
    );
    assert_eq!(child.id(), Some(runtime_pid));
    assert_eq!(unsafe { libc::kill(terminal_pid, 0) }, 0);
    assert_eq!(
        std::fs::read_to_string(checkout.join("dirty.txt")).unwrap(),
        "retained uncommitted work"
    );
    let client = api.client().await;
    let proof = checkout.join("retained-env");
    client.client.call(zuse_rpc::methods::WRITE_TERMINAL,json!({"terminalId":terminal["id"],"data":format!("printf '%s' \"$MEMORY_RETAINED\" > '{}'\n",proof.display())})).await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while std::fs::read_to_string(&proof).unwrap_or_default() != "yes" {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(std::fs::read_to_string(&proof).unwrap(), "yes");
    let mut messages = client
        .client
        .subscribe(
            zuse_rpc::methods::WATCH_DOC_MESSAGES,
            json!({"chatId":"session-bob"}),
        )
        .await
        .unwrap();
    let initial = tokio::time::timeout(Duration::from_secs(5), messages.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        initial
            .to_string()
            .matches("Cloud runtime fixture reply")
            .count(),
        1,
        "resume must not replay the original turn"
    );
    let mut retained_messages = Vec::new();
    let initial: zuse_doc::TranscriptUpdate = serde_json::from_value(initial).unwrap();
    zuse_doc::apply_transcript_frame(&mut retained_messages, initial.frame).unwrap();
    client.client.call(zuse_rpc::methods::RELAY_COMMAND, json!({"chatId":"session-bob","entry":{"id":"memory-followup","issuedBy":"cloud-ui","issuedAt":chrono::Utc::now().timestamp_millis(),"status":"pending","payload":{"kind":"run","messageId":"memory-followup-message","request":{"prompt":"Continue after retained memory sleep","harness":"mock","model":"fixture","cwd":checkout,"sandbox":"workspace-write","autoApprove":false}}}})).await.unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let current: zuse_doc::TranscriptUpdate =
                serde_json::from_value(messages.recv().await.unwrap()).unwrap();
            zuse_doc::apply_transcript_frame(&mut retained_messages, current.frame).unwrap();
            let count = serde_json::to_string(&retained_messages)
                .unwrap()
                .matches("Cloud runtime fixture reply")
                .count();
            assert!(count <= 2, "retained continuation duplicated execution");
            if count == 2 {
                break;
            }
        }
    })
    .await
    .unwrap();
    drop(messages);
    client.closed.cancel();
    drop(client);
    stop_guest(&mut child).await;
    assert_eq!(
        unsafe { libc::kill(terminal_pid, 0) },
        -1,
        "final shutdown must clean retained terminals"
    );
    api.finish().await;
}
