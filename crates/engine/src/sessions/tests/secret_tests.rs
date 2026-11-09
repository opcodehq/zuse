use super::*;
use zuse_providers::{HarnessError, RunIdentity};
struct PrivateExecution {
    secret: String,
    // The driver's private receipt is not a presentation event.
    receipt: std::path::PathBuf,
}
#[async_trait::async_trait]
impl Harness for PrivateExecution {
    fn id(&self) -> HarnessId {
        HarnessId::Mock
    }
    fn display_name(&self) -> &str {
        "Private execution fixture"
    }
    fn supports_steering(&self) -> bool {
        false
    }
    fn steering_mode(&self) -> zuse_protocol::SteeringMode {
        zuse_protocol::SteeringMode::TurnBoundary
    }
    fn reasoning_levels(&self) -> &[zuse_protocol::ReasoningLevel] {
        &[]
    }
    async fn models(&self) -> Result<Vec<zuse_protocol::Model>, HarnessError> {
        Ok(vec![])
    }
    async fn run(
        &self,
        _: RunRequest,
        _: RunControls,
    ) -> Result<futures::stream::BoxStream<'static, Result<AgentEvent, HarnessError>>, HarnessError>
    {
        panic!("identity required")
    }
    async fn run_delivery(
        &self,
        request: RunRequest,
        _: RunControls,
        identity: RunIdentity,
    ) -> Result<futures::stream::BoxStream<'static, Result<AgentEvent, HarnessError>>, HarnessError>
    {
        assert_eq!(
            request.prompt, self.secret,
            "operational prompt must remain unchanged"
        );
        assert_eq!(identity.chat_id, "secret-chat");
        assert_eq!(identity.message_id, "same-delivery");
        if self.receipt.exists() {
            assert_eq!(
                std::fs::read_to_string(&self.receipt).unwrap(),
                identity.message_id
            );
            return Ok(
                futures::stream::iter(vec![Ok(AgentEvent::DeliveryAcknowledged {
                    message_id: identity.message_id,
                })])
                .boxed(),
            );
        }
        // A real process needs the ORIGINAL argument and environment to work.
        // Only returned observational events are filtered, never command inputs.
        let result = tokio::process::Command::new("/bin/sh")
            .args([
                "-c",
                "test \"$1\" = \"$PROJECT_TOKEN\" || exit 91; printf x >> \"$2\"; printf '%s' \"$1\"",
                "_",
                &self.secret,
            ])
            .arg(self.receipt.with_extension("actions"))
            .env("PROJECT_TOKEN", &self.secret)
            .output()
            .await?;
        assert!(result.status.success());
        assert_eq!(result.stdout, self.secret.as_bytes());
        std::fs::write(&self.receipt, &identity.message_id)?;
        let output = format!("{} {}", self.secret, "large output ".repeat(20000));
        Ok(futures::stream::iter(vec![
            Ok(AgentEvent::SessionStarted {
                harness: HarnessId::Mock,
                model: "fixture".into(),
                tools: vec![],
                cwd: request.cwd,
                session_id: "private-session".into(),
                assistant_message_id: "assistant".into(),
            }),
            Ok(AgentEvent::ToolCall {
                id: "stable-tool".into(),
                call: zuse_protocol::ToolCall::Exec {
                    command: format!("echo {}", self.secret),
                },
            }),
            Ok(AgentEvent::ToolResult {
                id: "stable-tool".into(),
                is_error: false,
                output: Some(output),
                diff: None,
            }),
            Ok(AgentEvent::TextDelta {
                text: "private-".into(),
            }),
            Ok(AgentEvent::TextDelta {
                text: "credential".into(),
            }),
            Ok(AgentEvent::Done {
                status: DoneStatus::Completed,
                result: Some(self.secret.clone()),
                error: None,
                session_id: Some("private-session".into()),
            }),
        ])
        .boxed())
    }
}
#[tokio::test]
async fn private_execution_keeps_arguments_and_delivery_identity_but_journal_docs_sidecars_are_masked()
 {
    let dir = tempfile::tempdir().unwrap();
    let secret = "private-credential";
    let registry = HarnessRegistry::new();
    let secrets = Arc::new(zuse_providers::credentials::redaction::Secrets::default());
    secrets.record(secret).unwrap();
    registry.set_secret_filter(secrets.clone());
    let receipt = dir.path().join("private-receipt");
    registry.register(Arc::new(PrivateExecution {
        secret: secret.into(),
        receipt: receipt.clone(),
    }));
    let core =
        crate::EngineCore::assemble(dir.path(), Arc::new(registry), HarnessId::Mock, None).unwrap();
    core.sessions.set_secret_filter(secrets.clone());
    core.doc_host.set_secret_filter(secrets);
    for _ in 0..2 {
        let mut req = request();
        req.prompt = secret.into();
        core.sessions
            .dispatch(
                "secret-chat",
                HarnessId::Mock,
                req,
                Some("same-delivery".into()),
            )
            .await
            .unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while core.sessions.has_run("secret-chat") {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }
    assert_eq!(
        std::fs::read_to_string(receipt.with_extension("actions")).unwrap(),
        "x"
    );
    assert_eq!(std::fs::read_to_string(receipt).unwrap(), "same-delivery");
    let (replay, _) = core.sessions.subscribe("secret-chat", 0).unwrap();
    let wire = serde_json::to_string(&replay.iter().map(|e| &e.event).collect::<Vec<_>>()).unwrap();
    assert!(!wire.contains(secret));
    assert!(wire.contains("[redacted]"));
    assert_eq!(
        replay
            .iter()
            .filter(|e| matches!(e.event, AgentEvent::ToolCall { .. }))
            .count(),
        1
    );
    // DeliveryAcknowledged is consumed by the host ledger, not exposed as a
    // new public turn. The same durable delivery produced only one ToolCall.
    let handle = core.doc_host.open_local("secret-chat").unwrap();
    let messages = handle.doc().read_entries().unwrap();
    let json = serde_json::to_string(&messages).unwrap();
    assert!(!json.contains(secret));
    let output_ref = messages
        .iter()
        .flat_map(|m| &m.parts)
        .find_map(|part| match part {
            MessagePart::Tool { output_ref, .. } => output_ref.clone(),
            _ => None,
        })
        .unwrap();
    let output = core.doc_host.fetch_tool_blob(&output_ref).await.unwrap();
    assert!(output.len() > 128 * 1024);
    assert!(!output.contains(secret));
    assert!(output.contains("[redacted]"));
    // Explicit native-CLI import bypasses the live provider stream. Both
    // imported presentation rows and full sidecars still cross the same guard.
    let mut imported = messages.clone();
    for message in &mut imported {
        for part in &mut message.parts {
            if let MessagePart::Text { text, .. } = part {
                *text = secret.into();
            }
        }
    }
    core.doc_host.mask_imported_messages(&mut imported);
    assert!(!serde_json::to_string(&imported).unwrap().contains(secret));
    let raw = zuse_doc::SidecarPayload {
        part_id: "native-import-tool".into(),
        output: Some(secret.into()),
        diff: None,
    };
    let (reference, _) = core
        .doc_host
        .persist_tool_sidecar("secret-chat", "import", &raw)
        .unwrap();
    assert_eq!(
        raw.output.as_deref(),
        Some(secret),
        "caller retains original private input"
    );
    assert_eq!(
        core.doc_host
            .fetch_tool_blob(&reference.unwrap())
            .await
            .unwrap(),
        "[redacted]"
    );
    core.doc_host.shutdown_workers().await;
}
