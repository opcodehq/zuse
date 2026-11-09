//! New worktrees start from the remote's latest commits: the base is fetched
//! first, `HEAD` means the remote's default branch, unpushed local commits
//! win over the remote, and an unreachable remote falls back to the local
//! ref without failing creation.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};

use zuse_protocol::WorktreeProgress;

fn git(cwd: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .env("GIT_AUTHOR_NAME", "test")
        .env("GIT_AUTHOR_EMAIL", "test@test")
        .env("GIT_COMMITTER_NAME", "test")
        .env("GIT_COMMITTER_EMAIL", "test@test")
        .output()
        .expect("git runs");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn commit(repo: &Path, message: &str) -> String {
    git(repo, &["commit", "--allow-empty", "-q", "-m", message]);
    git(repo, &["rev-parse", "HEAD"])
}

struct Fixture {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    remote: PathBuf,
    repo: PathBuf,
    upstream: PathBuf,
    repos: zuse_engine::Repos,
}

/// A bare `remote`, the user's `repo` clone, and an `upstream` clone that
/// lands commits on the remote behind the user's back.
fn fixture() -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let remote = root.join("remote.git");
    let seed = root.join("seed");
    std::fs::create_dir_all(&seed).unwrap();
    git(&seed, &["init", "-q", "-b", "main"]);
    commit(&seed, "initial");
    git(&root, &["clone", "-q", "--bare", "seed", "remote.git"]);
    git(&root, &["clone", "-q", "remote.git", "repo"]);
    git(&root, &["clone", "-q", "remote.git", "upstream"]);
    let repos = zuse_engine::Repos::with_worktrees_root(
        &root.join("settings"),
        "test-device",
        root.join("worktrees"),
    );
    Fixture {
        repo: root.join("repo"),
        upstream: root.join("upstream"),
        remote,
        root,
        repos,
        _tmp: tmp,
    }
}

impl Fixture {
    fn push_upstream(&self, message: &str) -> String {
        let sha = commit(&self.upstream, message);
        git(&self.upstream, &["push", "-q", "origin", "HEAD"]);
        sha
    }

    async fn create(&self, base: &str) -> (String, Vec<WorktreeProgress>) {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = seen.clone();
        let worktree = self
            .repos
            .create_worktree_with_progress(&self.repo, base, move |progress| {
                sink.lock().unwrap().push(progress)
            })
            .await
            .unwrap();
        let path = PathBuf::from(&worktree.path);
        assert!(
            git(&path, &["config", "--get-regexp", "^branch\\."])
                .lines()
                .all(|line| !line.starts_with(&format!("branch.{}.", worktree.branch))),
            "the fresh branch must not track the base's upstream"
        );
        let head = git(&path, &["rev-parse", "HEAD"]);
        (head, seen.lock().unwrap().clone())
    }
}

fn phases(branch: &str, offline: bool) -> Vec<WorktreeProgress> {
    vec![
        WorktreeProgress::Fetching {
            branch: branch.into(),
        },
        WorktreeProgress::Creating {
            branch: branch.into(),
            offline,
        },
    ]
}

#[tokio::test(flavor = "multi_thread")]
async fn head_base_starts_from_the_fetched_default_branch() {
    let f = fixture();
    let local_main = git(&f.repo, &["rev-parse", "main"]);
    // The main checkout sits on an older feature branch.
    git(&f.repo, &["checkout", "-q", "-b", "feature"]);
    commit(&f.repo, "feature work");
    let latest = f.push_upstream("landed upstream");

    let (head, progress) = f.create("HEAD").await;

    assert_eq!(head, latest);
    assert_eq!(progress, phases("main", false));
    // Only the remote-tracking ref moves; the user's local main is untouched.
    assert_eq!(git(&f.repo, &["rev-parse", "main"]), local_main);
}

#[tokio::test(flavor = "multi_thread")]
async fn named_branch_behind_its_remote_uses_the_remote_commit() {
    let f = fixture();
    let latest = f.push_upstream("landed upstream");

    let (head, progress) = f.create("main").await;

    assert_eq!(head, latest);
    assert_eq!(progress, phases("main", false));
}

#[tokio::test(flavor = "multi_thread")]
async fn unpushed_local_commits_win_over_the_remote() {
    let f = fixture();
    f.push_upstream("landed upstream");
    let local = commit(&f.repo, "unpushed local work");

    let (head, _) = f.create("main").await;

    assert_eq!(head, local);
}

#[tokio::test(flavor = "multi_thread")]
async fn unreachable_remote_falls_back_to_the_local_branch() {
    let f = fixture();
    let local = git(&f.repo, &["rev-parse", "main"]);
    std::fs::remove_dir_all(&f.remote).unwrap();

    let (head, progress) = f.create("HEAD").await;

    assert_eq!(head, local);
    assert_eq!(progress, phases("main", true));
}

#[tokio::test(flavor = "multi_thread")]
async fn repository_without_remotes_skips_the_fetch() {
    let f = fixture();
    let solo = f.root.join("solo");
    std::fs::create_dir_all(&solo).unwrap();
    git(&solo, &["init", "-q", "-b", "main"]);
    let initial = commit(&solo, "initial");
    let seen = Arc::new(Mutex::new(Vec::new()));
    let sink = seen.clone();

    let worktree = f
        .repos
        .create_worktree_with_progress(&solo, "HEAD", move |progress| {
            sink.lock().unwrap().push(progress)
        })
        .await
        .unwrap();

    assert_eq!(
        git(Path::new(&worktree.path), &["rev-parse", "HEAD"]),
        initial
    );
    assert_eq!(
        *seen.lock().unwrap(),
        vec![WorktreeProgress::Creating {
            branch: "HEAD".into(),
            offline: false,
        }]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn option_shaped_base_is_never_fetched() {
    let f = fixture();
    let marker = f.root.join("injected");
    let base = format!("origin/--upload-pack=touch {}", marker.display());
    let seen = Arc::new(Mutex::new(Vec::new()));
    let sink = seen.clone();

    let result = f
        .repos
        .create_worktree_with_progress(&f.repo, &base, move |progress| {
            sink.lock().unwrap().push(progress)
        })
        .await;

    assert!(result.is_err());
    assert!(!marker.exists());
    assert!(
        seen.lock()
            .unwrap()
            .iter()
            .all(|progress| !matches!(progress, WorktreeProgress::Fetching { .. }))
    );
}
