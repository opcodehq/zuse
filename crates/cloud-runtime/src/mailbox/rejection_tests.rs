//! A refusal made before any side effect is definitive; anything else that
//! fails during admission stays uncertain.
use super::*;

/// Fails admission the way the engine executor would.
enum Failing {
    /// A check that ran before anything was written.
    Validation(&'static str),
    /// A failure after a write (an imported file, a settings change).
    AfterWrite,
}
#[async_trait]
impl Executor for Failing {
    async fn admitted(&self, _: &MessageSend, _: &str) -> Result<bool> {
        Ok(false)
    }
    async fn native_admitted(&self, _: &CloudSessionCommand, _: &str) -> Result<bool> {
        Ok(false)
    }
    async fn admit_native(&self, _: &CloudSessionCommand, _: &str, _: i64) -> Result<()> {
        match self {
            Self::Validation(code) => {
                require(false, code)?;
                unreachable!()
            }
            Self::AfterWrite => bail!("cloud_asset_import_failed"),
        }
    }
    async fn admit(&self, _: &MessageSend, _: &str, _: i64) -> Result<()> {
        match self {
            Self::Validation(code) => Err(reject(code)),
            Self::AfterWrite => bail!("engine_queue_write_failed"),
        }
    }
}
fn lease(key: &CommandKey, id: &str) -> RuntimeLease {
    let command: CloudSessionCommand = serde_json::from_value(json!({"commandId":id,"sessionId":"session","payload":{"kind":"run","messageId":format!("message-{id}"),"request":{"prompt":"fixture","cwd":".","harness":"codex","model":"fixture","sandbox":"workspace-write","attachments":["/outside/uploads.png"]}}})).unwrap();
    let plain = serde_json::to_vec(&command).unwrap();
    let mut envelope:CommandEnvelope=serde_json::from_value(json!({"protocolVersion":3,"workspaceId":"workspace","sessionId":"session","commandId":id,"kind":"zuse.session.run","fingerprint":key.fingerprint(&plain),"schemaVersion":1,"keyVersion":1,"destructionFence":0,"createdAt":now_ms(),"iv":"","ciphertext":"","dependencies":[]})).unwrap();
    (envelope.iv, envelope.ciphertext) = key.encrypt(&envelope, &plain).unwrap();
    RuntimeLease {
        command: envelope,
        workspace_sequence: 1,
        lease_token: format!("lease-{id}"),
        lease_deadline: now_ms() + 60000,
        runtime_generation: 1,
        provider_sandbox_id: "machine".into(),
        storage_incarnation_id: "disk".into(),
    }
}
fn mailbox(dir: &Path, executor: Failing) -> (Mailbox, CommandKey) {
    let database = dir.join("receipts.sqlite3");
    drop(Connection::open(&database).unwrap());
    let boot:CloudRuntimeBootstrap=serde_json::from_value(json!({"workspaceId":"workspace","zuseAccountId":"owner","chatId":"thread","initialSessionId":"session","runtimeCredential":"synthetic","runtimeGeneration":1,"gatewayEpoch":1,"runtimeCredentialExpiresAt":9007199254740000i64,"gatewayUrl":"wss://fixture.invalid","gatewayProtocol":"fixture","sealedTranscriptKey":"synthetic","runtimeStorage":{"format":"zuse-rust-engine-v1","storageIncarnationId":"disk"},"providerSandboxId":"machine","commandAdmissionVersion":2,"commandDeliveryMode":"server-admitted-v2"})).unwrap();
    let key = CommandKey::parse(&B64.encode([23; 32])).unwrap();
    let mailbox = Mailbox::new(
        &database,
        &boot,
        CommandKey::parse(&B64.encode([23; 32])).unwrap(),
        Arc::new(executor),
    )
    .unwrap();
    (mailbox, key)
}

#[tokio::test]
async fn validation_refusals_acknowledge_rejected_with_their_code() {
    for code in [
        "cloud_command_upload_outside_uploads",
        "cloud_command_upload_missing",
        "cloud_command_assets_too_many",
        "cloud_command_assets_too_large",
        "cloud_command_agent_changed",
    ] {
        let dir = tempfile::tempdir().unwrap();
        let (mailbox, key) = mailbox(dir.path(), Failing::Validation(code));
        let lease = lease(&key, "refused");
        let ack = mailbox.apply(&lease).await.unwrap().unwrap();
        assert!(matches!(ack.state, AcknowledgmentState::Rejected), "{code}");
        assert_eq!(ack.category.as_deref(), Some(code));
        assert!(ack.result_ciphertext.is_none());
        // The receipt is durable: a redelivery replays the same refusal and
        // never runs admission again.
        let again = mailbox.apply(&lease).await.unwrap().unwrap();
        assert!(matches!(again.state, AcknowledgmentState::Rejected));
        assert_eq!(again.category.as_deref(), Some(code));
    }
}

#[tokio::test]
async fn a_failure_after_a_write_stays_outcome_unknown() {
    let dir = tempfile::tempdir().unwrap();
    let (mailbox, key) = mailbox(dir.path(), Failing::AfterWrite);
    let ack = mailbox
        .apply(&lease(&key, "uncertain"))
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(ack.state, AcknowledgmentState::OutcomeUnknown));
    assert_eq!(
        ack.category.as_deref(),
        Some("runtime-admission-not-proven")
    );
}

#[test]
fn a_refusal_keeps_its_code_through_added_context() {
    let error = reject("cloud_command_upload_missing").context("binding a task command");
    assert_eq!(
        error.downcast_ref::<AdmissionRejected>(),
        Some(&AdmissionRejected("cloud_command_upload_missing"))
    );
    assert!(
        anyhow::anyhow!("cloud_command_upload_missing")
            .downcast_ref::<AdmissionRejected>()
            .is_none(),
        "only the typed refusal is definitive, never a matching string"
    );
}
