use super::*;

struct Capture(Arc<Mutex<Vec<String>>>);
#[async_trait::async_trait]
impl Harness for Capture {
    fn id(&self) -> HarnessId {
        HarnessId::Mock
    }
    fn display_name(&self) -> &str {
        "Provider context fixture"
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
    async fn models(&self) -> Result<Vec<zuse_protocol::Model>, zuse_providers::HarnessError> {
        Ok(vec![])
    }
    async fn run(
        &self,
        request: RunRequest,
        _: RunControls,
    ) -> Result<
        futures::stream::BoxStream<'static, Result<AgentEvent, zuse_providers::HarnessError>>,
        zuse_providers::HarnessError,
    > {
        lock(&self.0).push(request.prompt);
        Ok(futures::stream::iter(vec![Ok(AgentEvent::Done {
            status: DoneStatus::Completed,
            result: None,
            error: None,
            session_id: None,
        })])
        .boxed())
    }
}

#[tokio::test]
async fn provider_context_reaches_agent_without_changing_user_message_or_retry_request() {
    let dir = tempfile::tempdir().unwrap();
    let prompts = Arc::new(Mutex::new(Vec::new()));
    let registry = Arc::new(HarnessRegistry::new());
    registry.register(Arc::new(Capture(prompts.clone())));
    let core = crate::EngineCore::assemble(dir.path(), registry, HarnessId::Mock, None).unwrap();
    core.sessions
        .set_provider_context(Arc::new(|request| {
            request
                .prompt
                .push_str("\nWait for checkout before accessing repository files.");
            Ok(())
        }))
        .unwrap();
    core.sessions
        .dispatch(
            "context-chat",
            HarnessId::Mock,
            request(),
            Some("original-message".into()),
        )
        .await
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while core.sessions.has_run("context-chat") {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        *lock(&prompts),
        vec!["first\nWait for checkout before accessing repository files."]
    );
    assert_eq!(
        lock(&core.sessions.inner.last_requests)["context-chat"].prompt,
        "first"
    );
    let handle = core.doc_host.open_local("context-chat").unwrap();
    let entries = handle.doc().read_entries().unwrap();
    let user = entries
        .iter()
        .find(|entry| entry.id == "original-message")
        .unwrap();
    let text: String = user
        .parts
        .iter()
        .filter_map(|part| match part {
            MessagePart::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(text, "first");
    core.shutdown().await;
}
