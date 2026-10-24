//! Single-test process: scoped GitHub fixture PATH and real checkout operations.
#![cfg(unix)]
use serde_json::{Value, json};
use std::{os::unix::fs::PermissionsExt, path::Path, sync::Arc};
use zuse_engine::{EngineCore, HarnessRegistry};
use zuse_protocol::{
    HarnessId,
    creation_source::{self as wire, Resolution, Row, Source, Tab},
};

fn git(root: &Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().into()
}

#[tokio::test]
async fn sources_prepare_reuse_replay_and_attach_without_a_chat_or_primary_checkout_change() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("project");
    std::fs::create_dir(&root).unwrap();
    git(&root, &["init", "-q", "-b", "main"]);
    git(&root, &["config", "user.name", "Source acceptance"]);
    git(&root, &["config", "user.email", "source@example.test"]);
    std::fs::write(root.join("README.md"), "Primary checkout\n").unwrap();
    git(&root, &["add", "README.md"]);
    git(&root, &["commit", "-qm", "Initial"]);
    git(&root, &["remote", "add", "origin", root.to_str().unwrap()]);
    for branch in ["feature/available", "feature/reused", "pr-fixture"] {
        git(&root, &["branch", branch]);
    }
    git(
        &root,
        &["update-ref", "refs/remotes/origin/feature/remote", "HEAD"],
    );
    let reused = temp.path().join("reused");
    git(
        &root,
        &[
            "worktree",
            "add",
            reused.to_str().unwrap(),
            "feature/reused",
        ],
    );
    std::fs::write(reused.join("uncommitted.txt"), "Keep these bytes").unwrap();
    std::fs::write(root.join("README.md"), "Unsaved primary changes\n").unwrap();
    let before = git(&root, &["status", "--porcelain=v1"]);
    let head = git(&root, &["rev-parse", "HEAD"]);
    let bin = temp.path().join("bin");
    std::fs::create_dir(&bin).unwrap();
    let gh = bin.join("gh");
    std::fs::write(
        &gh,
        include_str!("../../../scripts/acceptance/fixtures/creation-sources-gh.py"),
    )
    .unwrap();
    std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o755)).unwrap();
    // This test has its own binary and is its only test.
    unsafe {
        std::env::set_var(
            "PATH",
            format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
        );
        std::env::set_var("ZUSE_NO_LOGIN_SHELL", "1");
        std::env::set_var("ZUSE_CREATION_SOURCE_FIXTURE", temp.path());
    }
    let core = EngineCore::assemble(
        &temp.path().join("profile"),
        Arc::new(HarnessRegistry::new()),
        HarnessId::Mock,
        None,
    )
    .unwrap();
    let client = zuse_rpc::memory_client(core.rpc_service().clone());
    let info = client.call("EngineInfo", json!({})).await.unwrap();
    client.call("AddRepo", json!({"path":root})).await.unwrap();
    let registered: Vec<Row> = client
        .call_as(wire::LIST, json!({"repoPath":root,"tab":"branches"}))
        .await
        .unwrap();
    assert!(
        registered
            .iter()
            .any(|row| row.title == "feature/available"),
        "A registered repository can be used before its first chat or Space"
    );
    client.call("Mutate",json!({"op":"createSpace","spaceId":"source-project","deviceId":info["deviceId"],"path":root,"name":"Source project","gitDetected":true})).await.unwrap();
    client
        .call(
            "SetWorktreeSettings",
            json!({"useCustomDirectory":true,"customDirectory":temp.path().join("worktrees")}),
        )
        .await
        .unwrap();
    let list = |tab| json!({"repoPath":root,"tab":tab});
    let rows: Vec<Row> = client
        .call_as(wire::LIST, list(Tab::Branches))
        .await
        .unwrap();
    assert!(!rows.iter().any(|r| r.title == "main"));
    assert!(rows.iter().any(|r| r.title == "feature/reused" && r.in_use));
    assert!(rows.iter().any(|r| r.title == "feature/remote"));
    let request = |source| json!({"repoPath":root,"spaceId":"source-project","source":source,"requestId":uuid::Uuid::new_v4().to_string()});
    let cloud_request = |source| {
        let mut value = request(source);
        value["cloud"] = json!(true);
        value
    };
    let checkouts_before = git(&root, &["worktree", "list", "--porcelain"]);
    let unpublished = cloud_request(Source::Branch {
        reference: "refs/heads/feature/available".into(),
    });
    assert!(
        client
            .call(wire::RESOLVE, unpublished)
            .await
            .unwrap_err()
            .to_string()
            .contains("Push")
    );
    git(
        &root,
        &[
            "update-ref",
            "refs/remotes/origin/feature/available",
            "HEAD",
        ],
    );
    let cloud_branch_request = cloud_request(Source::Branch {
        reference: "refs/heads/feature/available".into(),
    });
    let cloud_branch: Value = client
        .call(wire::RESOLVE, cloud_branch_request.clone())
        .await
        .unwrap();
    assert_eq!(cloud_branch["kind"], "cloudRef");
    assert_eq!(cloud_branch["baseRef"], "origin/feature/available");
    assert_eq!(cloud_branch["branch"], "feature/available");
    assert_eq!(
        client
            .call(wire::RESOLVE, cloud_branch_request.clone())
            .await
            .unwrap(),
        cloud_branch
    );
    let mut different_destination = cloud_branch_request;
    different_destination["cloud"] = json!(false);
    assert!(
        client
            .call(wire::RESOLVE, different_destination)
            .await
            .is_err()
    );
    let cloud_remote: Value = client
        .call(
            wire::RESOLVE,
            cloud_request(Source::Branch {
                reference: "refs/remotes/origin/feature/remote".into(),
            }),
        )
        .await
        .unwrap();
    assert_eq!(cloud_remote["baseRef"], "origin/feature/remote");
    let cloud_pr: Value = client
        .call(wire::RESOLVE, cloud_request(Source::Pr { number: 72 }))
        .await
        .unwrap();
    assert_eq!(cloud_pr["baseRef"], "origin/feature/from-pr");
    assert_eq!(cloud_pr["branch"], "feature/from-pr");
    let cloud_fork: Value = client
        .call(wire::RESOLVE, cloud_request(Source::Pr { number: 84 }))
        .await
        .unwrap();
    assert_eq!(cloud_fork["baseRef"], "refs/pull/84/head");
    assert!(cloud_fork["branch"].is_null());
    let cloud_issue: Value = client
        .call(wire::RESOLVE, cloud_request(Source::Issue { number: 93 }))
        .await
        .unwrap();
    assert!(cloud_issue["uploadId"].is_string());
    assert!(
        cloud_issue["markdown"]
            .as_str()
            .unwrap()
            .contains("Preserve the text")
    );
    assert_eq!(
        git(&root, &["worktree", "list", "--porcelain"]),
        checkouts_before,
        "Cloud sources cannot prepare a local worktree"
    );
    assert_eq!(git(&root, &["branch", "--show-current"]), "main");
    let selected = request(Source::Branch {
        reference: "refs/heads/feature/available".into(),
    });
    let first: Resolution = client
        .call_as(wire::RESOLVE, selected.clone())
        .await
        .unwrap();
    let Resolution::Checkout { outcome, .. } = first else {
        panic!("Checkout expected")
    };
    assert_eq!(outcome.worktree.branch, "feature/available");
    assert_ne!(Path::new(&outcome.worktree.path), root);
    assert_eq!(
        git(Path::new(&outcome.worktree.path), &["rev-parse", "HEAD"]),
        head
    );
    let replay: Value = client.call(wire::RESOLVE, selected.clone()).await.unwrap();
    assert_eq!(replay["outcome"]["path"], outcome.worktree.path);
    let mut conflict = selected.clone();
    conflict["source"] = json!({"kind":"branch","reference":"refs/heads/pr-fixture"});
    assert!(client.call(wire::RESOLVE, conflict).await.is_err());
    let reused_reply: Value = client
        .call(
            wire::RESOLVE,
            request(Source::Branch {
                reference: "refs/heads/feature/reused".into(),
            }),
        )
        .await
        .unwrap();
    assert_eq!(reused_reply["outcome"]["path"], reused.to_str().unwrap());
    assert_eq!(
        std::fs::read_to_string(reused.join("uncommitted.txt")).unwrap(),
        "Keep these bytes"
    );
    // A remote-only source creates a tracking branch; it does not fall back to HEAD.
    let remote: Value = client
        .call(
            wire::RESOLVE,
            request(Source::Branch {
                reference: "refs/remotes/origin/feature/remote".into(),
            }),
        )
        .await
        .unwrap();
    assert_eq!(remote["outcome"]["branch"], "feature/remote");
    std::fs::write(temp.path().join("fail-pr-list"), "").unwrap();
    assert!(
        client
            .call(wire::LIST, list(Tab::PullRequests))
            .await
            .is_err()
    );
    let prs: Vec<Row> = client
        .call_as(wire::LIST, list(Tab::PullRequests))
        .await
        .unwrap();
    assert_eq!(prs.len(), 2);
    let pr: Value = client
        .call(wire::RESOLVE, request(Source::Pr { number: 72 }))
        .await
        .unwrap();
    assert_eq!(pr["outcome"]["branch"], "feature/from-pr");
    let fork: Value = client
        .call(wire::RESOLVE, request(Source::Pr { number: 84 }))
        .await
        .unwrap();
    assert_eq!(fork["outcome"]["branch"], "bob/main");
    assert_ne!(fork["outcome"]["path"], root.to_str().unwrap());
    let issue_request = request(Source::Issue { number: 93 });
    let issue: Value = client
        .call(wire::RESOLVE, issue_request.clone())
        .await
        .unwrap();
    assert_eq!(
        client.call(wire::RESOLVE, issue_request).await.unwrap(),
        issue
    );
    let attachment = client
        .call(
            "ReadAttachmentChunk",
            json!({"path":issue["attachmentPath"],"offset":0}),
        )
        .await
        .unwrap();
    use base64::Engine as _;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(attachment["data"].as_str().unwrap())
        .unwrap();
    assert!(
        String::from_utf8(bytes)
            .unwrap()
            .contains("Preserve the text and attach this issue")
    );
    let calls = std::fs::read_to_string(temp.path().join("github-calls.jsonl")).unwrap();
    assert_eq!(
        calls
            .lines()
            .filter(|line| line.contains("\"issue\", \"view\""))
            .count(),
        2,
        "Each local or Cloud issue request reads its issue once, including receipt retries"
    );
    assert_eq!(git(&root, &["branch", "--show-current"]), "main");
    assert_eq!(git(&root, &["status", "--porcelain=v1"]), before);
    assert_eq!(
        std::fs::read_to_string(root.join("README.md")).unwrap(),
        "Unsaved primary changes\n"
    );
    assert!(core.workspace.read_chats().unwrap().is_empty());
    assert!(
        client
            .call(wire::LIST, json!({"repoPath":temp.path(),"tab":"branches"}))
            .await
            .is_err()
    );
}
