//! `GetBranchGraph` over real repositories: divergence, fork point, and the
//! `git merge-tree` conflict preview.
use serde_json::{Value, json};
use std::{path::Path, sync::Arc};
use zuse_engine::{EngineCore, HarnessRegistry};
use zuse_protocol::HarnessId;
use zuse_rpc::{RpcClient, methods};
fn git_as(root: &Path, author: &str, args: &[&str]) -> String {
    let email = format!("{}@localhost", author.to_lowercase().replace(' ', "."));
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(root)
        .env("GIT_AUTHOR_NAME", author)
        .env("GIT_AUTHOR_EMAIL", &email)
        .env("GIT_COMMITTER_NAME", author)
        .env("GIT_COMMITTER_EMAIL", &email)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().into()
}
fn git(root: &Path, args: &[&str]) -> String {
    git_as(root, "Fixture", args)
}
fn commit(root: &Path, author: &str, file: &str, text: &str, message: &str) -> String {
    std::fs::write(root.join(file), text).unwrap();
    git_as(root, author, &["add", file]);
    git_as(root, author, &["commit", "-qm", message]);
    git(root, &["rev-parse", "HEAD"])
}
/// Git 2.38+ previews merges; older Git reports no preview.
fn merge_tree_supported(root: &Path) -> bool {
    std::process::Command::new("git")
        .args(["merge-tree", "--write-tree", "HEAD", "HEAD"])
        .current_dir(root)
        .output()
        .is_ok_and(|out| out.status.success())
}
struct Fixture {
    root: tempfile::TempDir,
    _core: EngineCore,
    client: RpcClient,
}
impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let repo = root.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "-q", "-b", "main"]);
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
        Self {
            root,
            _core: core,
            client,
        }
    }
    fn repo(&self) -> std::path::PathBuf {
        self.root.path().join("repo")
    }
    async fn graph(&self, params: Value) -> Result<Value, zuse_rpc::RpcError> {
        let mut params = params;
        params["cwd"] = json!(self.repo());
        self.client.call(methods::GET_BRANCH_GRAPH, params).await
    }
}
#[tokio::test]
async fn diverged_branch_reports_counts_fork_and_conflict() {
    let f = Fixture::new();
    let repo = f.repo();
    let fork = commit(&repo, "Fixture", "app.txt", "one\ntwo\nthree\n", "Initial");
    git(&repo, &["checkout", "-qb", "feat"]);
    let feat_a = commit(
        &repo,
        "Fixture",
        "app.txt",
        "one\ntwo (feat)\nthree\n",
        "Feat edit",
    );
    let feat_b = commit(&repo, "Fixture", "feat.txt", "feat\n", "Feat file");
    git(&repo, &["checkout", "-q", "main"]);
    commit(
        &repo,
        "Other Dev",
        "app.txt",
        "one\ntwo (main)\nthree\n",
        "Main edit",
    );
    let main_tip = commit(&repo, "Other Dev", "other.txt", "other\n", "Main file");
    git(&repo, &["checkout", "-q", "feat"]);
    // An origin that already has main, so the base resolves to `origin/main`.
    let origin = f.root.path().join("origin.git");
    git(
        f.root.path(),
        &["clone", "-q", "--bare", "repo", "origin.git"],
    );
    git(
        &repo,
        &["remote", "add", "origin", origin.to_str().unwrap()],
    );
    git(&repo, &["fetch", "-q", "origin"]);
    std::fs::write(repo.join("scratch.txt"), "dirty\n").unwrap();
    let status_before = git(&repo, &["status", "--porcelain"]);

    let graph = f
        .graph(json!({"base":"main","fetch":true,"limit":10}))
        .await
        .unwrap();
    assert_eq!(graph["baseRef"], "origin/main");
    assert_eq!(graph["headBranch"], "feat");
    assert_eq!(graph["headSha"], feat_b);
    assert_eq!(graph["forkSha"], fork);
    assert_eq!(graph["ahead"], 2);
    assert_eq!(graph["behind"], 2);
    assert_eq!(graph["selfEmail"], "fixture@localhost");
    assert_eq!(graph["uncommitted"], 1);
    assert_eq!(graph["headShas"], json!([feat_b, feat_a]));
    let commits = graph["commits"].as_array().unwrap();
    assert_eq!(commits.len(), 5);
    assert_eq!(commits.last().unwrap()["sha"], fork);
    assert_eq!(commits.last().unwrap()["parentShas"], json!([]));
    let main_row = commits.iter().find(|c| c["sha"] == main_tip).unwrap();
    assert_eq!(main_row["authorName"], "Other Dev");
    assert_eq!(
        main_row["refs"],
        json!([{"kind":"branch","label":"main"},{"kind":"remote","label":"origin/main"}])
    );
    assert_eq!(
        commits[0]["refs"],
        json!([{"kind":"branch","label":"feat"},{"kind":"remote","label":"origin/feat"}])
    );
    if merge_tree_supported(&repo) {
        let merge = &graph["merge"];
        assert_eq!(merge["clean"], false, "{merge}");
        assert_eq!(merge["moreFiles"], 0);
        assert_eq!(
            merge["conflicts"],
            json!([{
                "path": "app.txt",
                "baseAuthor": "Other Dev",
                "hunks": [{"line": 2, "head": "two (feat)", "base": "two (main)"}],
                "note": null,
            }])
        );
    } else {
        assert!(graph["merge"].is_null());
    }
    // Read-only: the checkout, the index and the branch stay put.
    assert_eq!(git(&repo, &["status", "--porcelain"]), status_before);
    assert_eq!(git(&repo, &["rev-parse", "HEAD"]), feat_b);
    assert_eq!(git(&repo, &["rev-parse", "--abbrev-ref", "HEAD"]), "feat");

    let missing = f.graph(json!({"base":"nope"})).await.unwrap_err();
    assert!(missing.to_string().contains("nope"), "{missing}");
}
#[tokio::test]
async fn merged_base_keeps_the_original_fork_and_previews_clean() {
    let f = Fixture::new();
    let repo = f.repo();
    let fork = commit(&repo, "Fixture", "app.txt", "one\n", "Initial");
    git(&repo, &["checkout", "-qb", "feat"]);
    commit(&repo, "Fixture", "feat.txt", "feat\n", "Feat file");
    git(&repo, &["checkout", "-q", "main"]);
    commit(&repo, "Other Dev", "other.txt", "other\n", "Main file");
    git(&repo, &["checkout", "-q", "feat"]);
    git(&repo, &["merge", "-q", "--no-edit", "main"]);
    let graph = f.graph(json!({"base":"main"})).await.unwrap();
    assert_eq!(graph["baseRef"], "main");
    assert_eq!(graph["forkSha"], fork);
    assert_eq!(graph["ahead"], 2);
    assert_eq!(graph["behind"], 0);
    assert_eq!(graph["headShas"].as_array().unwrap().len(), 2);
    assert_eq!(graph["commits"].as_array().unwrap().len(), 4);
    assert_eq!(graph["uncommitted"], 0);
    if merge_tree_supported(&repo) {
        assert_eq!(
            graph["merge"],
            json!({"clean": true, "conflicts": [], "moreFiles": 0})
        );
    }
}
