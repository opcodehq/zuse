use super::*;
use futures::{StreamExt, stream::BoxStream};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use zuse_engine::{HarnessRegistry, TitleGenerator};
use zuse_protocol::{
    AgentEvent, DoneStatus, HarnessId, Model, ReasoningLevel, RunRequest, SteeringMode, methods,
};
use zuse_providers::{Harness, HarnessError, RunControls};
use zuse_rpc::RpcService;

struct RecordingTitles(AtomicUsize);
#[async_trait::async_trait]
impl Harness for RecordingTitles {
    fn id(&self) -> HarnessId {
        HarnessId::Codex
    }
    fn display_name(&self) -> &str {
        "Adoption title regression; no vendor calls"
    }
    fn supports_steering(&self) -> bool {
        false
    }
    fn steering_mode(&self) -> SteeringMode {
        SteeringMode::TurnBoundary
    }
    fn reasoning_levels(&self) -> &[ReasoningLevel] {
        &[]
    }
    async fn models(&self) -> Result<Vec<Model>, HarnessError> {
        panic!("explicit title model required")
    }
    async fn run(
        &self,
        _: RunRequest,
        _: RunControls,
    ) -> Result<BoxStream<'static, Result<AgentEvent, HarnessError>>, HarnessError> {
        panic!("this title proof never executes a coding turn")
    }
    async fn run_title(
        &self,
        _: RunRequest,
        _: RunControls,
    ) -> Result<BoxStream<'static, Result<AgentEvent, HarnessError>>, HarnessError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(futures::stream::iter(vec![
            Ok(AgentEvent::TextDelta {
                text: "Generated Control Title".into(),
            }),
            Ok(AgentEvent::Done {
                status: DoneStatus::Completed,
                result: None,
                error: None,
                session_id: None,
            }),
        ])
        .boxed())
    }
}
async fn create(core: &EngineCore, checkout: &std::path::Path, parent: &str, initial: &str) {
    core.rpc_service().handle(methods::MUTATE,serde_json::json!({"op":"createChat","chatId":parent,"deviceId":core.device_id,"cwd":checkout,"config":{"harness":"codex","model":"fixture","sandbox":"workspace-write"}})).await.unwrap();
    if initial != parent {
        core.rpc_service()
            .handle(
                methods::CREATE_THREAD_SESSION,
                serde_json::json!({"chatId":initial,"threadId":parent}),
            )
            .await
            .unwrap();
    }
}
fn title(core: &EngineCore, id: &str) -> Option<String> {
    core.workspace.chat(id).unwrap().unwrap().title
}

#[tokio::test]
async fn fresh_parent_and_distinct_blank_initial_session_are_named_before_dispatch() {
    let dir = tempfile::tempdir().unwrap();
    let core = EngineCore::assemble(
        dir.path(),
        Arc::new(HarnessRegistry::new()),
        HarnessId::Mock,
        None,
    )
    .unwrap();
    create(&core, dir.path(), "parent", "initial").await;
    core.workspace.rename_chat("initial", " \t ").unwrap();
    seed_adoption_titles(&core, "parent", "initial", "Admitted task title").unwrap();
    assert_eq!(
        title(&core, "parent").as_deref(),
        Some("Admitted task title")
    );
    assert_eq!(
        title(&core, "initial").as_deref(),
        Some("Admitted task title"),
        "the dispatched session must not launch an automatic title request"
    );
    core.shutdown().await;
}

#[tokio::test]
async fn existing_titles_survive_adoption_and_same_identity_is_supported() {
    let dir = tempfile::tempdir().unwrap();
    let core = EngineCore::assemble(
        dir.path(),
        Arc::new(HarnessRegistry::new()),
        HarnessId::Mock,
        None,
    )
    .unwrap();
    create(&core, dir.path(), "parent", "initial").await;
    core.workspace
        .rename_chat("parent", "User named parent")
        .unwrap();
    core.workspace
        .rename_chat("initial", "User named session")
        .unwrap();
    seed_adoption_titles(&core, "parent", "initial", "Admitted task title").unwrap();
    assert_eq!(title(&core, "parent").as_deref(), Some("User named parent"));
    assert_eq!(
        title(&core, "initial").as_deref(),
        Some("User named session")
    );
    create(&core, dir.path(), "same", "same").await;
    seed_adoption_titles(&core, "same", "same", "Same identity title").unwrap();
    assert_eq!(title(&core, "same").as_deref(), Some("Same identity title"));
    core.shutdown().await;
}

#[tokio::test(flavor = "current_thread")]
async fn seeded_parent_and_initial_session_skip_real_title_generator_with_positive_control() {
    let dir = tempfile::tempdir().unwrap();
    let registry = Arc::new(HarnessRegistry::new());
    let recorder = Arc::new(RecordingTitles(AtomicUsize::new(0)));
    registry.register(recorder.clone());
    let core = EngineCore::assemble(dir.path(), registry.clone(), HarnessId::Codex, None).unwrap();
    registry
        .set_title_settings(zuse_engine::registry::TitleSettings {
            harness: Some(HarnessId::Codex),
            model: Some("fixture-title-model".into()),
        })
        .unwrap();
    create(&core, dir.path(), "parent", "initial").await;
    create(&core, dir.path(), "control", "control").await;
    seed_adoption_titles(&core, "parent", "initial", "Admitted task title").unwrap();
    let generator = TitleGenerator::new(core.workspace.clone(), registry, core.repos.clone());
    for id in ["parent", "initial", "control"] {
        generator.maybe_generate(
            id,
            HarnessId::Codex,
            "The admitted first prompt",
            &dir.path().to_string_lossy(),
            zuse_engine::titles::TitleTrigger::TurnCompleted,
        );
    }
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            if title(&core, "control").is_some_and(|title| !title.trim().is_empty()) {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        recorder.0.load(Ordering::SeqCst),
        1,
        "only the intentionally untitled control may invoke the title harness"
    );
    assert_eq!(
        title(&core, "parent").as_deref(),
        Some("Admitted task title")
    );
    assert_eq!(
        title(&core, "initial").as_deref(),
        Some("Admitted task title")
    );
    core.shutdown().await;
}
