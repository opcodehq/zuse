//! Approval callbacks exercise the same engine bridge used by native providers.
use async_trait::async_trait;
use futures::{StreamExt, stream::BoxStream};
use std::{sync::Arc, time::Duration};
use zuse_engine::{EngineCore, HarnessRegistry};
use zuse_protocol::{
    AgentEvent, DoneStatus, HarnessId, Model, ReasoningLevel, RunRequest, SandboxLevel,
    SteeringMode, UserInputAnswer, UserInputQuestion,
    permission::{self, ToolApproval},
};
use zuse_providers::{Harness, HarnessError, RunControls};

struct Approvals(HarnessId);
#[async_trait]
impl Harness for Approvals {
    fn id(&self) -> HarnessId {
        self.0
    }
    fn display_name(&self) -> &str {
        "Approval fixture"
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
        Ok(vec![])
    }
    async fn run(
        &self,
        request: RunRequest,
        controls: RunControls,
    ) -> Result<BoxStream<'static, Result<AgentEvent, HarnessError>>, HarnessError> {
        let (tx, rx) = tokio::sync::mpsc::channel(8);
        tokio::spawn(async move {
            for index in 0..2 {
                let content = request.prompt == "content";
                let mut approval = ToolApproval::new(
                    "fixture:exec",
                    serde_json::json!({"command":"cargo test"}),
                    "Yes",
                    "No",
                );
                approval.force_prompt = request.prompt == "forced";
                if request.prompt == "reserved" {
                    approval.deny_label = permission::ALLOW_PROJECT.into();
                }
                let options = vec![approval.allow_label.clone(), approval.deny_label.clone()];
                let question = UserInputQuestion {
                    id: format!("q{index}"),
                    approval: (!content).then_some(approval),
                    header: "Approval".into(),
                    question: "Allow action?".into(),
                    options,
                    multi_select: false,
                    prefill: None,
                    multiline: false,
                };
                let answers = (controls.request_input)(vec![question])
                    .await
                    .unwrap_or_default();
                let answer = answers
                    .first()
                    .and_then(|a| a.labels.first())
                    .cloned()
                    .unwrap_or_default();
                let _ = tx.send(Ok(AgentEvent::TextDelta { text: answer })).await;
            }
            let _ = tx
                .send(Ok(AgentEvent::Done {
                    status: DoneStatus::Completed,
                    result: None,
                    error: None,
                    session_id: None,
                }))
                .await;
        });
        Ok(futures::stream::unfold(rx, |mut rx| async move {
            rx.recv().await.map(|event| (event, rx))
        })
        .boxed())
    }
}
fn assemble(dir: &std::path::Path) -> EngineCore {
    let registry = Arc::new(HarnessRegistry::new());
    registry.register(Arc::new(Approvals(HarnessId::Mock)));
    EngineCore::assemble(dir, registry, HarnessId::Mock, None).unwrap()
}
async fn run(
    core: &EngineCore,
    cwd: &str,
    mode: SandboxLevel,
    prompt: &str,
    pick: Option<&str>,
) -> (usize, Vec<String>, Vec<Vec<String>>) {
    run_on(core, cwd, mode, prompt, pick, HarnessId::Mock).await
}
async fn run_on(
    core: &EngineCore,
    cwd: &str,
    mode: SandboxLevel,
    prompt: &str,
    pick: Option<&str>,
    harness: HarnessId,
) -> (usize, Vec<String>, Vec<Vec<String>>) {
    let chat = uuid::Uuid::new_v4().to_string();
    core.doc_host.open(&chat).unwrap();
    let (_, mut events) = core.sessions.subscribe(&chat, 0).unwrap();
    let request: RunRequest = serde_json::from_value(
        serde_json::json!({"cwd":cwd,"sandbox":mode,"prompt":prompt,"autoApprove":true}),
    )
    .unwrap();
    core.sessions
        .dispatch(&chat, harness, request, None)
        .await
        .unwrap();
    let mut prompts = 0;
    let mut replies = vec![];
    let mut options = vec![];
    loop {
        match tokio::time::timeout(Duration::from_secs(10), events.recv())
            .await
            .unwrap()
            .unwrap()
            .event
        {
            AgentEvent::InputRequested {
                request_id,
                questions,
            } => {
                prompts += 1;
                assert!(
                    questions.iter().all(|question| question.approval.is_none()),
                    "raw matching inputs stay on the host"
                );
                options.push(questions[0].options.clone());
                let question_id = questions[0].id.clone();
                assert!(
                    core.sessions
                        .respond_input("foreign", &request_id, vec![])
                        .unwrap()
                        == false
                );
                core.sessions
                    .respond_input(
                        &chat,
                        &request_id,
                        vec![UserInputAnswer {
                            question_id,
                            labels: vec![pick.unwrap_or("No").into()],
                        }],
                    )
                    .unwrap();
            }
            AgentEvent::TextDelta { text } => replies.push(text),
            AgentEvent::Done { .. } => break,
            _ => {}
        }
    }
    (prompts, replies, options)
}
#[tokio::test]
async fn project_grant_saved_from_prompt_survives_engine_restart_and_read_only_wins() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_str().unwrap();
    {
        let core = assemble(dir.path());
        let (prompts, replies, options) = run(
            &core,
            cwd,
            SandboxLevel::WorkspaceWrite,
            "tool",
            Some(permission::ALLOW_PROJECT),
        )
        .await;
        assert_eq!(prompts, 1);
        assert_eq!(replies, ["Yes", "Yes"]);
        assert!(options[0].contains(&permission::ALLOW_CHAT.into()));
        core.shutdown().await;
    }
    let core = assemble(dir.path());
    let (prompts, replies, _) = run(&core, cwd, SandboxLevel::WorkspaceWrite, "tool", None).await;
    assert_eq!(prompts, 0);
    assert_eq!(replies, ["Yes", "Yes"]);
    let (prompts, replies, _) = run(&core, cwd, SandboxLevel::ReadOnly, "tool", None).await;
    assert_eq!(prompts, 0);
    assert_eq!(replies, ["No", "No"]);
}
#[tokio::test]
async fn chat_grant_does_not_leak_and_content_questions_never_create_rules() {
    let dir = tempfile::tempdir().unwrap();
    let core = assemble(dir.path());
    let cwd = dir.path().to_str().unwrap();
    let (prompts, replies, _) = run(
        &core,
        cwd,
        SandboxLevel::WorkspaceWrite,
        "tool",
        Some(permission::ALLOW_CHAT),
    )
    .await;
    assert_eq!(prompts, 1);
    assert_eq!(replies, ["Yes", "Yes"]);
    let (prompts, replies, _) =
        run(&core, cwd, SandboxLevel::WorkspaceWrite, "tool", Some("No")).await;
    assert_eq!(prompts, 2);
    assert_eq!(replies, ["No", "No"]);
    let (prompts, _, options) = run(
        &core,
        cwd,
        SandboxLevel::DangerFullAccess,
        "content",
        Some(permission::ALLOW_PROJECT),
    )
    .await;
    assert_eq!(prompts, 2);
    assert!(options.iter().all(|o| o == &["Yes", "No"]));
    let (prompts, _, _) = run(&core, cwd, SandboxLevel::WorkspaceWrite, "tool", Some("No")).await;
    assert_eq!(prompts, 2);
}
#[tokio::test]
async fn forged_persistent_answers_cannot_silence_forced_or_native_review_escalations() {
    let dir = tempfile::tempdir().unwrap();
    let core = assemble(dir.path());
    let cwd = dir.path().to_str().unwrap();
    for (mode, prompt) in [
        (SandboxLevel::WorkspaceWrite, "forced"),
        (SandboxLevel::Auto, "tool"),
    ] {
        let (prompts, replies, options) =
            run(&core, cwd, mode, prompt, Some(permission::ALLOW_PROJECT)).await;
        assert_eq!(prompts, 2);
        assert_eq!(replies, ["No", "No"]);
        assert!(options.iter().all(|o| o == &["Yes", "No"]));
    }
    let (prompts, replies, options) = run(
        &core,
        cwd,
        SandboxLevel::WorkspaceWrite,
        "reserved",
        Some(permission::ALLOW_PROJECT),
    )
    .await;
    assert_eq!(prompts, 2);
    assert_eq!(
        replies,
        [permission::ALLOW_PROJECT, permission::ALLOW_PROJECT]
    );
    assert!(options.iter().all(|options| options.len() == 2));
    let (prompts, _, _) = run(&core, cwd, SandboxLevel::WorkspaceWrite, "tool", Some("No")).await;
    assert_eq!(
        prompts, 2,
        "a provider's reserved label cannot create a grant"
    );
}

#[tokio::test]
async fn missing_or_unoffered_approval_answers_never_grant_access() {
    let dir = tempfile::tempdir().unwrap();
    let core = assemble(dir.path());
    let cwd = dir.path().to_str().unwrap();
    for label in ["yes", "Allow for everything", "", "Yes, always"] {
        let (prompts, replies, _) = run(
            &core,
            cwd,
            SandboxLevel::WorkspaceWrite,
            "tool",
            Some(label),
        )
        .await;
        assert_eq!(prompts, 2);
        assert_eq!(replies, ["No", "No"]);
    }
}

#[tokio::test]
async fn inspector_rpc_derives_project_scope_and_mutations_return_confirmed_snapshots() {
    use zuse_rpc::{RpcReply, RpcService};
    let dir = tempfile::tempdir().unwrap();
    let core = assemble(dir.path());
    let cwd = dir
        .path()
        .canonicalize()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    for chat in ["owner", "other"] {
        core.workspace
            .create_chat(chat, None, None, None, Some(cwd.clone()))
            .unwrap();
    }
    let project = format!("path:{cwd}");
    core.sessions
        .permission_rules()
        .save(
            &project,
            Some("owner"),
            &ToolApproval::new(
                "fixture:exec",
                serde_json::json!({"command":"cargo test"}),
                "Yes",
                "No",
            ),
        )
        .unwrap();
    let rpc = core.rpc_service();
    let RpcReply::Value(snapshot) = rpc
        .handle(permission::LIST, serde_json::json!({"chatId":"owner"}))
        .await
        .unwrap()
    else {
        panic!("unary snapshot")
    };
    let id = snapshot["rules"][0]["id"].as_str().unwrap();
    let RpcReply::Value(other) = rpc
        .handle(
            permission::LIST,
            serde_json::json!({"chatId":"other", "project":project}),
        )
        .await
        .unwrap()
    else {
        panic!("unary snapshot")
    };
    assert_eq!(other["rules"], serde_json::json!([]));
    assert!(
        rpc.handle(
            permission::REMOVE,
            serde_json::json!({"chatId":"other","id":id})
        )
        .await
        .is_err()
    );
    let RpcReply::Value(edited) = rpc
        .handle(
            permission::UPDATE,
            serde_json::json!({"chatId":"owner","id":id,"effect":"ask","chatOnly":true}),
        )
        .await
        .unwrap()
    else {
        panic!("unary snapshot")
    };
    assert_eq!(edited["rules"][0]["effect"], "ask");
    let RpcReply::Value(removed) = rpc
        .handle(
            permission::REMOVE,
            serde_json::json!({"chatId":"owner","id":id}),
        )
        .await
        .unwrap()
    else {
        panic!("unary snapshot")
    };
    assert_eq!(removed["rules"], serde_json::json!([]));
    assert_eq!(removed["activity"][0]["source"], "rule removed");
    assert!(
        rpc.handle(permission::LIST, serde_json::json!({"chatId":"missing"}))
            .await
            .is_err()
    );
    core.shutdown().await;
}

#[tokio::test]
async fn grants_bind_to_the_actual_runtime_even_when_request_harness_is_absent() {
    let dir = tempfile::tempdir().unwrap();
    let core = assemble(dir.path());
    core.registry
        .register(Arc::new(Approvals(HarnessId::Codex)));
    let cwd = dir.path().to_str().unwrap();
    let (prompts, replies, _) = run(
        &core,
        cwd,
        SandboxLevel::WorkspaceWrite,
        "tool",
        Some(permission::ALLOW_PROJECT),
    )
    .await;
    assert_eq!(prompts, 1);
    assert_eq!(replies, ["Yes", "Yes"]);
    let (prompts, replies, _) = run_on(
        &core,
        cwd,
        SandboxLevel::WorkspaceWrite,
        "tool",
        Some("No"),
        HarnessId::Codex,
    )
    .await;
    assert_eq!(prompts, 2);
    assert_eq!(replies, ["No", "No"]);
    core.shutdown().await;
}
