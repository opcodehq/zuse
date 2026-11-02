//! Real Git repositories and canonical RPC, including local remote push/pull.
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};
use zuse_engine::{EngineCore, HarnessRegistry};
use zuse_protocol::HarnessId;
use zuse_rpc::{RpcClient, methods};
fn git(root: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(root)
        .env("GIT_AUTHOR_NAME", "Fixture")
        .env("GIT_AUTHOR_EMAIL", "fixture@localhost")
        .env("GIT_COMMITTER_NAME", "Fixture")
        .env("GIT_COMMITTER_EMAIL", "fixture@localhost")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().into()
}
struct Fixture {
    root: tempfile::TempDir,
    repo: PathBuf,
    core: EngineCore,
    client: RpcClient,
}
impl Fixture {
    async fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let repo = root.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "-b", "main"]);
        git(&repo, &["config", "commit.gpgsign", "false"]);
        git(&repo, &["config", "user.name", "Fixture"]);
        git(&repo, &["config", "user.email", "fixture@localhost"]);
        let core = EngineCore::assemble(
            &root.path().join("profile"),
            Arc::new(HarnessRegistry::new()),
            HarnessId::Mock,
            None,
        )
        .unwrap();
        let client = zuse_rpc::memory_client(core.rpc_service());
        client.call(methods::MUTATE,json!({"op":"createSpace","spaceId":"space","deviceId":core.device_id,"path":repo,"name":"Fixture","gitDetected":true})).await.unwrap();
        client
            .call(
                methods::MUTATE,
                json!({"op":"createChat","chatId":"chat","spaceId":"space","cwd":repo}),
            )
            .await
            .unwrap();
        Self {
            root,
            repo,
            core,
            client,
        }
    }
    async fn status(&self) -> Value {
        self.client
            .call(
                methods::GET_GIT_WORKSPACE,
                json!({"chatId":"chat","cwd":self.repo}),
            )
            .await
            .unwrap()
    }
    async fn action_at(
        &self,
        status: &Value,
        operation: Value,
    ) -> Result<Value, zuse_rpc::RpcError> {
        self.client.call(methods::GIT_WORKSPACE_ACTION,json!({"chatId":"chat","cwd":self.repo,"checkoutId":status["checkoutId"],"revision":status["revision"],"operation":operation})).await
    }
    async fn action(&self, operation: Value) -> Value {
        self.action_at(&self.status().await, operation)
            .await
            .unwrap()
    }
}
#[tokio::test]
async fn literal_staging_unborn_unstage_selective_commit_stale_index_and_archive_admission() {
    let f = Fixture::new().await;
    std::fs::write(f.repo.join("literal[1].txt"), "literal\n").unwrap();
    std::fs::write(f.repo.join("literal1.txt"), "other\n").unwrap();
    let before = f.status().await;
    assert!(before["headSha"].is_null());
    let create = json!({"action":"createPullRequest","title":"Feature","body":"", "base":"", "baseRepository":null,"draft":true});
    let error = f.action_at(&before, create.clone()).await.unwrap_err();
    assert!(error.to_string().contains("Push this branch"));
    let staged = f
        .action(json!({"action":"stage","paths":["literal[1].txt"]}))
        .await;
    assert_eq!(
        git(&f.repo, &["diff", "--cached", "--name-only"]),
        "literal[1].txt"
    );
    f.action_at(
        &staged,
        json!({"action":"unstage","paths":["literal[1].txt"]}),
    )
    .await
    .unwrap();
    assert_eq!(git(&f.repo, &["ls-files"]), "");
    assert_eq!(
        std::fs::read_to_string(f.repo.join("literal[1].txt")).unwrap(),
        "literal\n"
    );
    f.action(json!({"action":"stage","paths":["literal[1].txt"]}))
        .await;
    f.action(json!({"action":"commit","message":"Initial selected file"}))
        .await;
    assert_eq!(
        git(&f.repo, &["ls-tree", "--name-only", "HEAD"]),
        "literal[1].txt"
    );
    std::fs::write(f.repo.join("literal[1].txt"), "unstaged edit\n").unwrap();
    let stale = f.status().await;
    git(&f.repo, &["add", "--", "literal1.txt"]);
    assert!(
        f.action_at(
            &stale,
            json!({"action":"commit","message":"Must reject stale index"})
        )
        .await
        .is_err()
    );
    f.action(json!({"action":"commit","message":"Preserve unstaged changes"}))
        .await;
    assert_eq!(git(&f.repo, &["show", "HEAD:literal[1].txt"]), "literal");
    assert_eq!(
        std::fs::read_to_string(f.repo.join("literal[1].txt")).unwrap(),
        "unstaged edit\n"
    );
    let current = f.status().await;
    let error = f.action_at(&current, create).await.unwrap_err();
    assert!(error.to_string().contains("Push this branch"));
    for path in ["../outside", "literal*.txt", "/tmp/outside"] {
        assert!(
            f.action_at(&current, json!({"action":"stage","paths":[path]}))
                .await
                .is_err()
        );
    }
    f.client
        .call(
            methods::MUTATE,
            json!({"op":"setChatArchived","chatId":"chat","archived":true}),
        )
        .await
        .unwrap();
    assert!(
        f.action_at(
            &current,
            json!({"action":"stage","paths":["literal[1].txt"]})
        )
        .await
        .is_err()
    );
    assert!(f.repo.exists());
    f.core.shutdown().await;
}
#[tokio::test]
async fn branch_push_pull_and_non_fast_forward_preserve_local_history() {
    let f = Fixture::new().await;
    std::fs::write(f.repo.join("file"), "initial").unwrap();
    git(&f.repo, &["add", "."]);
    git(&f.repo, &["commit", "-m", "Initial"]);
    let remote = f.root.path().join("remote.git");
    std::fs::create_dir(&remote).unwrap();
    git(&remote, &["init", "--bare", "-b", "main"]);
    git(
        &f.repo,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    let pushed = f.action(json!({"action":"push"})).await;
    assert_eq!(pushed["upstream"], "origin/main");
    let branch = f
        .action(json!({"action":"createBranch","name":"feature/local"}))
        .await;
    assert_eq!(branch["branch"], "feature/local");
    assert_eq!(
        f.core
            .workspace
            .chat("chat")
            .unwrap()
            .unwrap()
            .branch
            .as_deref(),
        Some("feature/local")
    );
    f.action(json!({"action":"switchBranch","name":"main"}))
        .await;
    let peer = f.root.path().join("peer");
    git(
        f.root.path(),
        &["clone", remote.to_str().unwrap(), peer.to_str().unwrap()],
    );
    std::fs::write(peer.join("peer-file"), "remote change").unwrap();
    git(&peer, &["add", "."]);
    git(&peer, &["commit", "-m", "Remote change"]);
    git(&peer, &["push"]);
    f.action(json!({"action":"pull"})).await;
    assert!(f.repo.join("peer-file").exists());
    std::fs::write(f.repo.join("local-file"), "local").unwrap();
    git(&f.repo, &["add", "."]);
    git(&f.repo, &["commit", "-m", "Local diverging commit"]);
    let local = git(&f.repo, &["rev-parse", "HEAD"]);
    std::fs::write(peer.join("another"), "remote").unwrap();
    git(&peer, &["add", "."]);
    git(&peer, &["commit", "-m", "Remote diverging commit"]);
    git(&peer, &["push"]);
    assert!(
        f.action_at(&f.status().await, json!({"action":"pull"}))
            .await
            .is_err()
    );
    assert_eq!(git(&f.repo, &["rev-parse", "HEAD"]), local);
    assert!(f.repo.join("local-file").exists());
    assert!(
        f.action_at(&f.status().await, json!({"action":"push"}))
            .await
            .is_err()
    );
    f.core.shutdown().await;
}
