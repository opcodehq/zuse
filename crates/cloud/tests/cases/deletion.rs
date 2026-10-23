// Explicit deletion acceptance shares the real archived guest and encrypted object fixture.
#[async_trait]
impl zuse_cloud::runtime::deletion::DeletionProvider for RecoveryMachine {
    async fn find(&self, id: &str) -> Result<Option<Machine>, ProviderError> {
        if id == "machine-bob" {
            return RecoveryProvider::inspect(self, id).await;
        }
        Ok(self
            .replacement
            .lock()
            .unwrap()
            .as_ref()
            .filter(|m| m.id == id || m.name == id)
            .cloned())
    }
    async fn destroy(&self, id: &str) -> Result<(), ProviderError> {
        if let Some(mut child) = self.helper.lock().await.take() {
            let _ = child.start_kill();
            let _ = child.wait().await;
        }
        if let Some(mut child) = self.runtime.lock().await.take() {
            let _ = child.start_kill();
            let _ = child.wait().await;
        }
        assert!(["machine-bob", "replacement-bob"].contains(&id));
        if id == "machine-bob" {
            self.destroyed.store(true, Ordering::SeqCst);
        } else {
            *self.replacement.lock().unwrap() = None;
        }
        self.retired.lock().unwrap().push(id.into());
        if self.source.exists() {
            std::fs::remove_dir_all(&self.source).unwrap();
        }
        if self.lose_delete_reply.swap(false, Ordering::SeqCst) {
            return Err(ProviderError::OutcomeUnknown);
        }
        Ok(())
    }
}
impl ArchiveFixture {
    async fn delete(&self, command: &str) -> Value {
        let (status, value) = runtime_post_for_url(
            &format!("{}/v1/cloud/workspaces/ws-bob/delete", self.api.origin),
            "bob",
            &json!({"workspaceId":"ws-bob","commandId":command}),
        )
        .await;
        assert_eq!(status, 200, "{value}");
        value
    }
    async fn delete_tick(&self) -> Result<bool, ApiError> {
        sqlx::query("UPDATE zuse_rust_jobs SET due_at=0 WHERE kind='workspace-delete'")
            .execute(&self.api.pool)
            .await
            .unwrap();
        self.control
            .delete_once(&self.machine, chrono::Utc::now().timestamp_millis())
            .await
    }
    async fn deleted(&self) -> bool {
        sqlx::query_scalar(
            "SELECT state='deleted' FROM api_cloud_workspaces WHERE workspace_id='ws-bob'",
        )
        .fetch_one(&self.api.pool)
        .await
        .unwrap()
    }
    async fn finish_delete(&self) {
        tokio::time::timeout(Duration::from_secs(10), async {
            while !self.deleted().await {
                self.delete_tick().await.unwrap();
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
    }
}
#[tokio::test]
#[ignore = "requires isolated PostgreSQL and packaged guest"]
async fn permanent_delete_purges_archives_and_ciphertext_then_keeps_a_content_free_thirty_day_tombstone()
 {
    let f = ArchiveFixture::start().await;
    f.control
        .begin_recovery("ws-bob", chrono::Utc::now().timestamp_millis())
        .await
        .unwrap()
        .unwrap();
    f.upload().await;
    f.tick().await.unwrap();
    f.tick().await.unwrap();
    assert_eq!(f.phase().await, "retired");
    let (status, _) = runtime_post_for_url(
        &format!("{}/v1/cloud/workspaces/ws-bob/delete", f.api.origin),
        "alice",
        &json!({"workspaceId":"ws-bob","commandId":"forged-delete"}),
    )
    .await;
    assert_eq!(status, 404);
    let before = sqlx::query_scalar::<_, i64>("SELECT count(*) FROM api_entitlements")
        .fetch_one(&f.api.pool)
        .await
        .unwrap();
    f.objects.inner.bytes.lock().unwrap().insert(
        "workspaces/ws-bob-lookalike/history".into(),
        b"unrelated workspace".to_vec(),
    );
    f.objects.inner.bytes.lock().unwrap().insert(
        "workspaces/ws-alice/history".into(),
        b"other account".to_vec(),
    );
    let value = f.delete("delete-retired-workspace").await;
    assert_eq!(value["state"], "deleting");
    assert_eq!(value["deletion"]["phase"], "providers");
    assert!(!f.deleted().await);
    let failed_read=reqwest::Client::new().get(format!("{}/v1/cloud/workspaces/ws-bob/sessions/session-bob/transcript-checkpoint?format=zuse-rust-transcript-v1",f.api.origin)).bearer_auth("bob").send().await.unwrap();
    assert!(!failed_read.status().is_success());
    // Provider cleanup precedes object cleanup; object errors leave the deletion retryable.
    f.delete_tick().await.unwrap();
    f.objects.inner.fail.store(true, Ordering::SeqCst);
    assert!(f.delete_tick().await.is_err());
    assert!(!f.deleted().await);
    sqlx::query("UPDATE zuse_rust_jobs SET state='failed' WHERE kind='workspace-delete'")
        .execute(&f.api.pool)
        .await
        .unwrap();
    f.delete("retry-the-same-delete").await;
    f.objects.inner.fail.store(false, Ordering::SeqCst);
    f.finish_delete().await;
    let row: Value = sqlx::query_scalar(
        "SELECT to_jsonb(w) FROM api_cloud_workspaces w WHERE workspace_id='ws-bob'",
    )
    .fetch_one(&f.api.pool)
    .await
    .unwrap();
    assert_eq!(row["desired_state"], "deleted");
    assert_eq!(
        row["request_config"]["rustDeleteOperation"]["phase"],
        "deleted"
    );
    assert!(row["wrapped_transcript_key"].is_null());
    assert!(row["runtime_credential_hash"].is_null());
    assert!(row["runtime_boot_token_hash"].is_null());
    assert!(row["provider_sandbox_id"].is_null());
    assert_eq!(
        row["deletion_tombstone_expires_at"].as_i64().unwrap()
            - row["deleted_at"].as_i64().unwrap(),
        zuse_cloud::runtime::deletion::TOMBSTONE_MS
    );
    assert_eq!(row["branch"], "");
    assert_eq!(row["base_ref"], "");
    assert!(row["request_config"].get("recoveryArchive").is_none());
    assert!(
        row["request_config"]
            .get("runtimeBootstrapReceipt")
            .is_none()
    );
    let stored = f.objects.inner.bytes.lock().unwrap();
    assert!(!stored.keys().any(|k| k.starts_with("workspaces/ws-bob/")));
    assert_eq!(
        stored.get("workspaces/ws-alice/history").unwrap(),
        b"other account"
    );
    assert_eq!(
        stored.get("workspaces/ws-bob-lookalike/history").unwrap(),
        b"unrelated workspace"
    );
    drop(stored);
    for table in [
        "api_cloud_transcript_checkpoints",
        "api_cloud_workspace_launch_intents",
        "zuse_rust_recovery_operations",
        "zuse_rust_restores",
        "zuse_rust_mailbox_commands",
        "zuse_rust_lifecycle_operations",
    ] {
        let count: i64 = sqlx::query_scalar(&format!(
            "SELECT count(*) FROM {table} WHERE workspace_id='ws-bob'"
        ))
        .fetch_one(&f.api.pool)
        .await
        .unwrap();
        assert_eq!(count, 0, "{table}");
    }
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM api_entitlements")
            .fetch_one(&f.api.pool)
            .await
            .unwrap(),
        before
    );
    assert!(
        !f.control
            .recovery_once(&f.machine, chrono::Utc::now().timestamp_millis())
            .await
            .unwrap()
    );
    assert!(
        !f.control
            .restore_once(&f.machine, chrono::Utc::now().timestamp_millis())
            .await
            .unwrap()
    );
    assert_eq!(
        f.delete("repeated-delete-after-completion").await["state"],
        "deleted"
    );
    // A delayed create retry with the original idempotency key cannot recreate deleted content.
    let (status,_)=runtime_post_for_url(&format!("{}/v1/cloud/workspaces",f.api.origin),"bob",&json!({"idempotencyKey":"ws-bob","projectId":"create-project","providerId":"boxd","agent":"claude","model":"fixture","baseRef":"main","branch":"late-create","firstMessage":"Do not resurrect"})).await;
    assert_eq!(status, 409);

    sqlx::query("UPDATE api_cloud_workspaces SET deletion_tombstone_expires_at=0 WHERE workspace_id='ws-bob'").execute(&f.api.pool).await.unwrap();
    assert_eq!(
        reqwest::Client::new()
            .get(format!("{}/v1/cloud/workspaces/ws-bob", f.api.origin))
            .bearer_auth("bob")
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
    f.finish().await;
}
#[tokio::test]
#[ignore = "requires isolated PostgreSQL and packaged guest"]
async fn delete_waits_for_a_live_verifier_and_cannot_be_undone_by_its_result() {
    let f = ArchiveFixture::start().await;
    f.control
        .begin_recovery("ws-bob", chrono::Utc::now().timestamp_millis())
        .await
        .unwrap()
        .unwrap();
    f.upload().await;
    f.objects.block.store(true, Ordering::SeqCst);
    sqlx::query("UPDATE zuse_rust_jobs SET due_at=0 WHERE kind='workspace-recovery'")
        .execute(&f.api.pool)
        .await
        .unwrap();
    {
        let current = f.control.clone();
        let verify = current.recovery_once(&f.machine, chrono::Utc::now().timestamp_millis());
        tokio::pin!(verify);
        tokio::select! {result=&mut verify=>panic!("verifier ended early: {result:?}"),_=f.objects.entered.notified()=>{}}
        f.delete("delete-while-verifying").await;
        f.delete_tick().await.unwrap();
        assert!(!f.deleted().await);
        assert!(f.machine.retired.lock().unwrap().is_empty());
        f.objects.release.notify_one();
        assert!(verify.await.is_err());
    }
    f.machine.lose_delete_reply.store(true, Ordering::SeqCst);
    assert!(f.delete_tick().await.is_err());
    assert!(!f.deleted().await);
    f.finish_delete().await;
    assert_eq!(&*f.machine.retired.lock().unwrap(), &["machine-bob"]);
    assert!(
        f.objects
            .inner
            .bytes
            .lock()
            .unwrap()
            .keys()
            .all(|k| !k.starts_with("workspaces/ws-bob/"))
    );
    f.finish().await;
}
#[tokio::test]
#[ignore = "requires isolated PostgreSQL and packaged guest"]
async fn delete_retires_a_partial_restore_target_without_launching_an_empty_guest() {
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
    f.tick().await.unwrap();
    f.unarchive("unarchive-partial-target").await;
    assert_eq!(
        runtime_post_for_url(
            &format!("{}/v1/cloud/workspaces/ws-bob/resume", f.api.origin),
            "bob",
            &json!({"workspaceId":"ws-bob","commandId":"partial-restore"})
        )
        .await
        .0,
        200
    );
    f.control
        .restore_once(&f.machine, chrono::Utc::now().timestamp_millis())
        .await
        .unwrap();
    sqlx::query("UPDATE zuse_rust_jobs SET due_at=0 WHERE kind='workspace-restore'")
        .execute(&f.api.pool)
        .await
        .unwrap();
    f.control
        .restore_once(&f.machine, chrono::Utc::now().timestamp_millis())
        .await
        .unwrap();
    f.delete("delete-partial-restore").await;
    f.finish_delete().await;
    assert!(f.machine.runtime.lock().await.is_none());
    assert!(f.machine.replacement.lock().unwrap().is_none());
    assert!(!f.machine.source.exists());
    assert!(
        !f.control
            .restore_once(&f.machine, chrono::Utc::now().timestamp_millis())
            .await
            .unwrap()
    );
    f.finish().await;
}

struct DeleteDuringAllocation {
    started: Notify,
    release: Notify,
    machine: std::sync::Mutex<Option<Machine>>,
    destroys: AtomicUsize,
}
#[async_trait]
impl zuse_cloud::runtime::allocation::AllocationProvider for DeleteDuringAllocation {
    async fn find(&self, id: &str) -> Result<Option<Machine>, ProviderError> {
        Ok(self
            .machine
            .lock()
            .unwrap()
            .as_ref()
            .filter(|m| m.id == id || m.name == id)
            .cloned())
    }
    async fn create(&self, _: &str, name: &str) -> Result<Machine, ProviderError> {
        self.started.notify_one();
        self.release.notified().await;
        let machine = Machine {
            id: "late-created-machine".into(),
            name: name.into(),
            state: MachineState::Running,
            vcpu: Some(2),
            memory_bytes: Some(1024 * 1024 * 1024),
        };
        *self.machine.lock().unwrap() = Some(machine.clone());
        Ok(machine)
    }
    async fn launch(
        &self,
        _: &str,
        _: &zuse_cloud::runtime::allocation::RuntimeLaunch,
    ) -> Result<(), ProviderError> {
        panic!("Deletion must fence the late guest launch")
    }
}
#[async_trait]
impl zuse_cloud::runtime::deletion::DeletionProvider for DeleteDuringAllocation {
    async fn find(&self, id: &str) -> Result<Option<Machine>, ProviderError> {
        zuse_cloud::runtime::allocation::AllocationProvider::find(self, id).await
    }
    async fn destroy(&self, id: &str) -> Result<(), ProviderError> {
        assert_eq!(id, "late-created-machine");
        self.destroys.fetch_add(1, Ordering::SeqCst);
        *self.machine.lock().unwrap() = None;
        Err(ProviderError::OutcomeUnknown)
    }
}
#[tokio::test]
#[ignore = "requires isolated PostgreSQL"]
async fn delete_waits_for_allocation_receipts_and_removes_the_late_machine_without_enrolling_it() {
    let api = GuestApi::start_with_projections().await;
    sqlx::raw_sql(include_str!("../fixtures/creation.sql"))
        .execute(&api.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE api_cloud_workspaces SET state='queued',desired_state='ready',runtime_state='offline',provider_sandbox_id=NULL,runtime_boot_token_hash=NULL,runtime_boot_token_expires_at=NULL,runtime_credential_hash=NULL,request_config='{\"runtimeGeneration\":2,\"gatewayEpoch\":5}' WHERE workspace_id='ws-bob'").execute(&api.pool).await.unwrap();
    api.control
        .queue_allocation(
            "ws-bob",
            "approved-rust-snapshot",
            chrono::Utc::now().timestamp_millis(),
        )
        .await
        .unwrap();
    let provider = DeleteDuringAllocation {
        started: Notify::new(),
        release: Notify::new(),
        machine: std::sync::Mutex::new(None),
        destroys: AtomicUsize::new(0),
    };
    {
        let allocator = api.control.clone();
        let allocation = allocator.allocate_once(&provider, chrono::Utc::now().timestamp_millis());
        tokio::pin!(allocation);
        tokio::select! {result=&mut allocation=>panic!("allocation ended early: {result:?}"),_=provider.started.notified()=>{}}
        let (status, value) = runtime_post_for_url(
            &format!("{}/v1/cloud/workspaces/ws-bob/delete", api.origin),
            "bob",
            &json!({"workspaceId":"ws-bob","commandId":"delete-during-create"}),
        )
        .await;
        assert_eq!(status, 200, "{value}");
        assert_eq!(value["state"], "deleting");
        api.control
            .delete_once(&provider, chrono::Utc::now().timestamp_millis())
            .await
            .unwrap();
        assert_eq!(provider.destroys.load(Ordering::SeqCst), 0);
        provider.release.notify_one();
        assert!(allocation.await.is_err());
    }
    let mut lost_reply = false;
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            sqlx::query("UPDATE zuse_rust_jobs SET due_at=0 WHERE kind='workspace-delete'")
                .execute(&api.pool)
                .await
                .unwrap();
            if let Err(error) = api
                .control
                .delete_once(&provider, chrono::Utc::now().timestamp_millis())
                .await
            {
                assert_eq!(error.code, "cloud_workspace_delete_failed");
                lost_reply = true;
            }
            let row: Value = sqlx::query_scalar(
                "SELECT to_jsonb(w) FROM api_cloud_workspaces w WHERE workspace_id='ws-bob'",
            )
            .fetch_one(&api.pool)
            .await
            .unwrap();
            assert!(row["provider_sandbox_id"].is_null());
            if row["state"] == "deleted" {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert!(lost_reply);
    assert_eq!(provider.destroys.load(Ordering::SeqCst), 1);
    assert!(provider.machine.lock().unwrap().is_none());
    api.finish().await;
}
