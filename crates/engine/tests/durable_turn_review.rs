//! Real engine dispatch/archive/restart with a deterministic file-writing provider.
use async_trait::async_trait;
use futures::{StreamExt, stream::BoxStream};
use serde_json::json;
use std::{path::Path, sync::Arc, time::Duration};
use zuse_engine::{EngineCore, HarnessRegistry};
use zuse_protocol::{AgentEvent, DoneStatus, HarnessId, Model, ReasoningLevel, RunRequest};
use zuse_providers::{Harness, HarnessError, RunControls};
use zuse_rpc::methods;
struct Writer;
#[async_trait]
impl Harness for Writer {
    fn id(&self) -> HarnessId {
        HarnessId::Mock
    }
    fn display_name(&self) -> &str {
        "Turn fixture"
    }
    fn supports_steering(&self) -> bool {
        false
    }
    fn steering_mode(&self) -> zuse_protocol::SteeringMode {
        zuse_protocol::SteeringMode::TurnBoundary
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
        _: RunControls,
    ) -> Result<BoxStream<'static, Result<AgentEvent, HarnessError>>, HarnessError> {
        // An immediate write proves capture completed before provider execution.
        std::fs::write(
            Path::new(&request.cwd).join("tracked.txt"),
            "user edit\nagent change\n",
        )
        .unwrap();
        std::fs::write(
            Path::new(&request.cwd).join("pre.txt"),
            "preexisting\nagent addition\n",
        )
        .unwrap();
        Ok(futures::stream::iter(vec![
            Ok(AgentEvent::TextDelta {
                text: "Done".into(),
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
fn git(root: &Path, args: &[&str]) {
    let result = std::process::Command::new("git")
        .args(args)
        .current_dir(root)
        .env("GIT_AUTHOR_NAME", "Fixture")
        .env("GIT_AUTHOR_EMAIL", "fixture@localhost")
        .env("GIT_COMMITTER_NAME", "Fixture")
        .env("GIT_COMMITTER_EMAIL", "fixture@localhost")
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&result.stderr)
    );
}
fn core(data: &Path) -> EngineCore {
    let registry = Arc::new(HarnessRegistry::new());
    registry.register(Arc::new(Writer));
    EngineCore::assemble(data, registry, HarnessId::Mock, None).unwrap()
}
#[tokio::test]
async fn latest_turn_survives_archive_restart_and_pruning_and_excludes_omitted_files() {
    let temp = tempfile::tempdir().unwrap();
    let data = temp.path().join("profile");
    let repo = temp.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-b", "main"]);
    std::fs::write(repo.join("tracked.txt"), "original\n").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-m", "Initial"]);
    let checkout;
    {
        let core = core(&data);
        let client = zuse_rpc::memory_client(core.rpc_service());
        client.call(methods::MUTATE,json!({"op":"createSpace","spaceId":"space","deviceId":core.device_id,"path":repo,"name":"Fixture","gitDetected":true})).await.unwrap();
        let tree = client
            .call(
                methods::CREATE_WORKTREE,
                json!({"repoPath":repo,"branch":"HEAD","spaceId":"space"}),
            )
            .await
            .unwrap();
        checkout = std::path::PathBuf::from(tree["path"].as_str().unwrap());
        client.call(methods::MUTATE,json!({"op":"createChat","chatId":"chat","spaceId":"space","cwd":checkout,"config":{"harness":"mock","sandbox":"workspace-write"}})).await.unwrap();
        core.workspace.rename_chat("chat", "Review turn").unwrap();
        std::fs::write(checkout.join("tracked.txt"), "user edit\n").unwrap();
        std::fs::write(checkout.join("pre.txt"), "preexisting\n").unwrap();
        std::fs::File::create(checkout.join("large-untracked.bin"))
            .unwrap()
            .set_len(9 * 1024 * 1024)
            .unwrap();
        let request: RunRequest = serde_json::from_value(
            json!({"prompt":"Change files","cwd":checkout,"sandbox":"workspace-write"}),
        )
        .unwrap();
        core.sessions
            .dispatch("chat", HarnessId::Mock, request, None)
            .await
            .unwrap();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
        while core.sessions.has_run("chat") {
            assert!(
                tokio::time::Instant::now() < deadline,
                "fixture did not finish"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let diff = client
            .call(
                methods::GET_CHECKOUT_DIFF,
                json!({"cwd":checkout,"chatId":"chat","mode":"turn"}),
            )
            .await
            .unwrap();
        assert!(diff["patch"].as_str().unwrap().contains("+agent change"));
        assert!(!diff["patch"].as_str().unwrap().contains("+user edit"));
        client
            .call(
                methods::MUTATE,
                json!({"op":"setChatArchived","chatId":"chat","archived":true}),
            )
            .await
            .unwrap();
        assert!(!checkout.exists());
        core.shutdown().await;
    }
    git(&repo, &["gc", "--prune=now"]);
    let core = core(&data);
    let client = zuse_rpc::memory_client(core.rpc_service());
    let diff = client
        .call(
            methods::GET_CHECKOUT_DIFF,
            json!({"cwd":checkout,"chatId":"chat","mode":"turn"}),
        )
        .await
        .unwrap();
    let files = diff["files"].as_array().unwrap();
    assert_eq!(files.len(), 2);
    assert!(!diff["patch"].as_str().unwrap().contains("large-untracked"));
    assert!(!diff["patch"].as_str().unwrap().contains("+preexisting"));
    let source=client.call(methods::GET_CHECKOUT_FILE_DIFF_TEXT,json!({"cwd":checkout,"chatId":"chat","mode":"turn","checkoutId":diff["checkoutId"],"diffChecksum":diff["checksum"],"path":"tracked.txt"})).await.unwrap();
    assert_eq!(source["oldText"], "user edit\n");
    assert_eq!(source["newText"], "user edit\nagent change\n");
    assert!(!checkout.exists());
    let pre=client.call(methods::GET_CHECKOUT_FILE_DIFF_TEXT,json!({"cwd":checkout,"chatId":"chat","mode":"turn","checkoutId":diff["checkoutId"],"diffChecksum":diff["checksum"],"path":"pre.txt"})).await.unwrap();
    assert_eq!(pre["oldText"], "preexisting\n");
    client
        .call(
            methods::MUTATE,
            json!({"op":"setChatArchived","chatId":"chat","archived":false}),
        )
        .await
        .unwrap();
    assert!(checkout.exists());
    let restored = client
        .call(
            methods::GET_CHECKOUT_DIFF,
            json!({"cwd":checkout,"chatId":"chat","mode":"turn"}),
        )
        .await
        .unwrap();
    assert!(
        restored["patch"]
            .as_str()
            .unwrap()
            .contains("+agent change")
    );
    core.diff_sync
        .note_turn_start("chat", temp.path().join("missing").to_str().unwrap())
        .await;
    assert!(core.diff_sync.turn_snapshot("chat").is_none());
    client
        .call(
            methods::DELETE_WORKTREE,
            json!({"repoPath":repo,"worktreePath":checkout}),
        )
        .await
        .unwrap();
    core.shutdown().await;
}
