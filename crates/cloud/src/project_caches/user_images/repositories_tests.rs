//! Repository baking: which repositories are planned, the repository set
//! digest, and the resumable step against the in-memory builder.
use super::boxd::BoxdUserImages;
use super::repositories::*;
use super::tests::{FakeHost, plan};
use super::*;
use serde_json::json;
use std::sync::{Arc, Mutex};

pub(super) fn repository(project: &str, name: &str, visibility: &str) -> PlannedRepository {
    PlannedRepository {
        project_id: project.into(),
        repository_identity: format!("github.com/{name}"),
        repository_url: format!("https://github.com/{name}.git"),
        default_branch: "main".into(),
        visibility: visibility.into(),
    }
}

#[test]
fn personal_images_plan_every_valid_repository_up_to_the_cap() {
    let mut all: Vec<_> = (0..MAX_REPOSITORIES + 2)
        .map(|n| {
            repository(
                &format!("project-{n}"),
                &format!("team/repo-{n}"),
                "private",
            )
        })
        .collect();
    all.push(PlannedRepository {
        repository_url: "https://evil.example/team/x.git".into(),
        ..repository("project-bad", "team/x", "public")
    });
    all.push(PlannedRepository {
        default_branch: "-x".into(),
        ..repository("project-branch", "team/y", "public")
    });
    let (planned, skipped) = select("user-alice", &all);
    assert_eq!(planned.len(), MAX_REPOSITORIES);
    let reasons: Vec<_> = skipped.iter().map(|s| s.reason.as_str()).collect();
    assert_eq!(
        reasons,
        [
            "repository_count_cap",
            "repository_count_cap",
            "repository_invalid",
            "repository_invalid"
        ]
    );
}

#[test]
fn organization_images_leave_private_repositories_out() {
    let all = [
        repository("p1", "team/app", "private"),
        repository("p2", "team/docs", "public"),
    ];
    let (planned, skipped) = select("organization:team", &all);
    assert_eq!(planned, vec![all[1].clone()]);
    assert_eq!(skipped[0].reason, "organization_private_repository");
}

#[test]
fn the_repository_set_digest_ignores_order_and_commits_but_not_membership() {
    let a = repository("p1", "team/app", "private");
    let b = repository("p2", "team/docs", "public");
    assert_eq!(set_digest(&[]), None);
    let digest = set_digest(&[a.clone(), b.clone()]).unwrap();
    assert_eq!(set_digest(&[b.clone(), a.clone()]).unwrap(), digest);
    assert_ne!(set_digest(std::slice::from_ref(&a)).unwrap(), digest);
    let renamed = PlannedRepository {
        default_branch: "trunk".into(),
        ..a.clone()
    };
    assert_ne!(set_digest(&[renamed, b]).unwrap(), digest);
    assert_eq!(
        path("github.com/Team/App"),
        path("github.com/team/app"),
        "one path per repository, whatever its case"
    );
    assert!(path("github.com/team/app").starts_with("/opt/zuse/repos/"));
    assert!(path("github.com/team/app").ends_with(".git"));
}

#[test]
fn identity_and_plan_carry_the_repository_set() {
    let base = plan("npm i -g pnpm");
    let key = base.identity.key().unwrap();
    let mut with = base.clone();
    with.identity.repositories_digest = set_digest(&[repository("p1", "team/app", "public")]);
    with.repositories = vec![repository("p1", "team/app", "public")];
    with.validate().unwrap();
    assert_ne!(with.identity.key().unwrap(), key);
    // Images planned before repositories keep their identity key.
    assert!(
        serde_json::to_value(&base.identity)
            .unwrap()
            .get("repositoriesDigest")
            .is_none()
    );
    let mut missing = with.clone();
    missing.identity.repositories_digest = None;
    assert!(missing.validate().is_err());
    let mut tampered = with.clone();
    tampered.repositories[0].repository_url = "https://github.com/other/app.git".into();
    assert!(tampered.validate().is_err());
}

#[derive(Default)]
pub(super) struct SourceState {
    snapshots: usize,
    discarded: usize,
    fail: Vec<SourceError>,
}
pub(super) struct FakeSource {
    root: tempfile::TempDir,
    bytes: u64,
    state: Mutex<SourceState>,
}
impl FakeSource {
    pub(super) fn new(bytes: u64) -> Arc<Self> {
        Arc::new(Self {
            root: tempfile::tempdir().unwrap(),
            bytes,
            state: Mutex::default(),
        })
    }
}
#[async_trait::async_trait]
impl RepositorySource for FakeSource {
    async fn snapshot(
        &self,
        _: &str,
        _: &str,
        repository: &PlannedRepository,
    ) -> Result<SourceSnapshot, SourceError> {
        let mut state = self.state.lock().unwrap();
        if !state.fail.is_empty() {
            return Err(state.fail.remove(0));
        }
        state.snapshots += 1;
        let file = self.root.path().join(repository.key());
        if !file.exists() {
            std::fs::write(&file, vec![7u8; self.bytes as usize]).unwrap();
        }
        Ok(SourceSnapshot {
            commit: "c".repeat(40),
            sha256: "d".repeat(64),
            bytes: self.bytes,
            file,
        })
    }
    fn discard(&self, _: &str) {
        self.state.lock().unwrap().discarded += 1;
    }
}

fn building(host: &FakeHost, repositories: Vec<PlannedRepository>) -> Build {
    let mut plan = plan("npm i -g pnpm");
    plan.identity.repositories_digest = set_digest(&repositories);
    plan.repositories = repositories;
    plan.skipped_repositories = vec![SkippedRepository {
        project_id: "p9".into(),
        repository_identity: "github.com/team/huge".into(),
        reason: "repository_count_cap".into(),
    }];
    plan.validate().unwrap();
    let machine = crate::providers::Machine {
        id: "vm-builder".into(),
        name: plan.names().0,
        state: crate::providers::MachineState::Running,
        vcpu: None,
        memory_bytes: None,
    };
    host.0
        .lock()
        .unwrap()
        .machines
        .insert(machine.id.clone(), (machine, "base-snapshot".into()));
    Build {
        image_id: plan.image_id.clone(),
        owner_id: plan.owner(),
        plan,
        phase: "repositories".into(),
        resources: json!({"effects":{},"provider":{"builderId":"vm-builder","pinsVerified":true,"setupStarted":true}}),
        artifact_id: None,
    }
}

async fn settle(
    backend: &BoxdUserImages,
    source: Option<&dyn RepositorySource>,
    build: &Build,
) -> ImageRepositories {
    let guest = backend
        .builder_guest(build)
        .expect("boxd exposes its builder");
    for _ in 0..32 {
        match step(guest.as_ref(), source, build).await {
            Ok(RepoStep::Done(record)) => return record,
            Ok(RepoStep::Pending) | Err(_) => {}
        }
    }
    panic!("repository step did not settle");
}

#[tokio::test]
async fn repositories_are_staged_in_chunks_unpacked_once_and_recorded() {
    let host = FakeHost::new();
    let backend = BoxdUserImages::new(Arc::new(host.clone()));
    let source = FakeSource::new(2 * UPLOAD_CHUNK as u64 + 10);
    let build = building(
        &host,
        vec![
            repository("p1", "team/app", "private"),
            repository("p2", "team/docs", "public"),
        ],
    );
    // A lost upload reply: the chunk landed, the next tick skips it.
    host.0.lock().unwrap().lose_reply = Some("upload");
    let record = settle(&backend, Some(source.as_ref()), &build).await;
    assert_eq!(record.baked.len(), 2);
    assert_eq!(record.baked[0].commit, "c".repeat(40));
    assert_eq!(record.baked[0].path, path("github.com/team/app"));
    assert_eq!(record.skipped.len(), 1, "planning-time skips are kept");
    let hit = record.find("github.com/team/app").unwrap();
    assert_eq!(hit.branch, "main");
    assert!(record.find("github.com/team/other").is_none());
    let state = host.0.lock().unwrap();
    for repository in ["github.com/team/app", "github.com/team/docs"] {
        let key = key(repository);
        assert_eq!(state.repos["baked"][&key]["commit"], "c".repeat(40));
    }
    // Every builder file outside the staging area is still a setup file.
    assert!(
        state
            .uploads
            .iter()
            .all(|p| p.starts_with("/opt/zuse/user-image/"))
    );
    assert!(source.state.lock().unwrap().discarded >= 1);
}

#[tokio::test]
async fn failing_repositories_are_retried_then_left_out_without_failing_the_build() {
    let host = FakeHost::new();
    let backend = BoxdUserImages::new(Arc::new(host.clone()));
    let source = FakeSource::new(100);
    source.state.lock().unwrap().fail = vec![
        SourceError::Skip("repository_access_unavailable"),
        SourceError::Retry("repository_fetch_failed"),
    ];
    host.0.lock().unwrap().unpack_failures = MAX_ATTEMPTS as usize;
    let build = building(
        &host,
        vec![
            repository("p1", "team/private", "private"),
            repository("p2", "team/flaky", "public"),
        ],
    );
    let record = settle(&backend, Some(source.as_ref()), &build).await;
    assert!(record.baked.is_empty(), "{record:?}");
    let reasons: Vec<_> = record
        .skipped
        .iter()
        .map(|s| (s.repository_identity.as_str(), s.reason.as_str()))
        .collect();
    assert_eq!(
        reasons,
        [
            ("github.com/team/huge", "repository_count_cap"),
            ("github.com/team/private", "repository_access_unavailable"),
            ("github.com/team/flaky", "repository_unpack_failed"),
        ]
    );
}

#[tokio::test]
async fn without_a_source_or_repositories_the_step_finishes_at_once() {
    let host = FakeHost::new();
    let backend = BoxdUserImages::new(Arc::new(host.clone()));
    let empty = building(&host, vec![]);
    let record = settle(&backend, None, &empty).await;
    assert!(record.baked.is_empty());
    assert!(host.0.lock().unwrap().commands.is_empty(), "no guest call");
    let build = building(&host, vec![repository("p1", "team/app", "public")]);
    let record = settle(&backend, None, &build).await;
    assert_eq!(
        record.skipped.last().unwrap().reason,
        "repository_source_unavailable"
    );
}

#[tokio::test]
async fn the_total_size_cap_leaves_later_repositories_out() {
    let host = FakeHost::new();
    let backend = BoxdUserImages::new(Arc::new(host.clone()));
    let source = FakeSource::new(10);
    let build = building(
        &host,
        vec![
            repository("p1", "team/app", "public"),
            repository("p2", "team/docs", "public"),
        ],
    );
    // The first repository already fills the image.
    host.0.lock().unwrap().repos = json!({"baked":{key("github.com/team/app"):{"baked":true,"commit":"a".repeat(40),"branch":"main","bytes":MAX_TOTAL_BYTES}}});
    let record = settle(&backend, Some(source.as_ref()), &build).await;
    assert_eq!(record.baked.len(), 1);
    assert_eq!(
        record.skipped.last().unwrap().reason,
        "repository_total_size_cap"
    );
}

#[tokio::test]
async fn git_snapshots_hold_full_history_or_the_tip_and_no_config() {
    // A local upstream stands in for GitHub.
    let dir = tempfile::tempdir().unwrap();
    let upstream = dir.path().join("team").join("app.git");
    std::fs::create_dir_all(&upstream).unwrap();
    let work = dir.path().join("work");
    let run = |args: &[&str], cwd: &std::path::Path| {
        let out = std::process::Command::new("git")
            .args(args)
            .current_dir(cwd)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "a")
            .env("GIT_AUTHOR_EMAIL", "a@example.com")
            .env("GIT_COMMITTER_NAME", "a")
            .env("GIT_COMMITTER_EMAIL", "a@example.com")
            .output()
            .unwrap();
        assert!(out.status.success(), "{args:?}");
        String::from_utf8(out.stdout).unwrap().trim().to_owned()
    };
    run(&["init", "-q", "--bare", "-b", "main"], &upstream);
    run(
        &["init", "-q", "-b", "main", work.to_str().unwrap()],
        dir.path(),
    );
    for n in 0..3 {
        std::fs::write(work.join("file.txt"), format!("{n}\n")).unwrap();
        run(&["add", "."], &work);
        run(&["commit", "-q", "-m", &format!("c{n}")], &work);
    }
    let head = run(&["rev-parse", "HEAD"], &work);
    run(&["push", "-q", upstream.to_str().unwrap(), "main"], &work);
    let list = |file: &std::path::Path| {
        let listing = std::process::Command::new("tar")
            .arg("-tf")
            .arg(file)
            .output()
            .unwrap();
        String::from_utf8(listing.stdout).unwrap()
    };
    let image = format!("uimg_{}", "2".repeat(32));
    // A small history is baked whole: no shallow boundary, so the runtime's
    // local clone of it is a file copy.
    let source = super::repository_source::GitRepositorySource::for_test(
        dir.path().join("cache"),
        format!("file://{}", dir.path().display()),
        64 * 1024 * 1024,
    );
    let made = source
        .snapshot(
            &image,
            "user-alice",
            &repository("p1", "team/app", "public"),
        )
        .await
        .unwrap();
    assert_eq!(made.commit, head);
    let listing = list(&made.file);
    assert!(!listing.lines().any(|l| l == "shallow"), "{listing}");
    assert!(listing.lines().any(|l| l.ends_with(".pack")), "{listing}");
    assert!(
        !listing
            .lines()
            .any(|l| l.starts_with("config") || l.starts_with("hooks"))
    );
    // A second call reuses the same snapshot.
    let again = source
        .snapshot(
            &image,
            "user-alice",
            &repository("p1", "team/app", "public"),
        )
        .await
        .unwrap();
    assert_eq!(again.sha256, made.sha256);
    source.discard(&image);
    assert!(!made.file.exists());
    // Over the full-history limit: only the tip, with its shallow boundary.
    let source = super::repository_source::GitRepositorySource::for_test(
        dir.path().join("cache"),
        format!("file://{}", dir.path().display()),
        1,
    );
    let tip = source
        .snapshot(
            &image,
            "user-alice",
            &repository("p1", "team/app", "public"),
        )
        .await
        .unwrap();
    assert_eq!(tip.commit, head);
    assert!(list(&tip.file).lines().any(|l| l == "shallow"));
}

#[tokio::test]
async fn a_missing_upstream_is_a_retryable_fetch_failure() {
    let dir = tempfile::tempdir().unwrap();
    let source = super::repository_source::GitRepositorySource::for_test(
        dir.path().join("cache"),
        format!("file://{}", dir.path().display()),
        64 * 1024 * 1024,
    );
    let image = format!("uimg_{}", "3".repeat(32));
    let error = source
        .snapshot(
            &image,
            "user-alice",
            &repository("p1", "team/absent", "public"),
        )
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        SourceError::Retry("repository_fetch_failed")
    ));
}

#[test]
fn a_full_staging_disk_is_named_and_git_stderr_is_summarized_safely() {
    use super::repository_source::{failure_reason, stderr_summary};
    let full = b"remote: Enumerating objects\nfatal: write error: No space left on device\nfatal: fetch-pack: invalid index-pack output\n";
    assert_eq!(failure_reason(full), "repository_staging_full");
    assert_eq!(
        failure_reason(b"fatal: repository 'x' not found\n"),
        "repository_fetch_failed"
    );
    assert_eq!(failure_reason(b""), "repository_fetch_failed");
    assert_eq!(
        stderr_summary(full),
        "fatal: fetch-pack: invalid index-pack output"
    );
    assert_eq!(stderr_summary(b"\n\n"), "");
    assert_eq!(
        stderr_summary(b"fatal: Authorization: Basic abc\n"),
        "[withheld]"
    );
    assert!(stderr_summary(&[b'a'; 1000]).len() <= 240);
    // Noisy progress ahead of the fatal line does not hide the cause.
    let mut noisy = b"remote: Counting objects\n".repeat(4096);
    noisy.extend_from_slice(full);
    super::repository_source::keep_tail(&mut noisy, 16 * 1024);
    assert_eq!(noisy.len(), 16 * 1024);
    assert_eq!(failure_reason(&noisy), "repository_staging_full");
}

#[test]
fn snapshots_use_the_configured_absolute_directory() {
    use super::repository_source::snapshot_root;
    assert_eq!(
        snapshot_root(Some("/var/lib/zuse/repos".into())),
        std::path::PathBuf::from("/var/lib/zuse/repos")
    );
    let fallback = std::env::temp_dir().join("zuse-user-image-repos");
    assert_eq!(snapshot_root(Some("relative/dir".into())), fallback);
    assert_eq!(snapshot_root(None), fallback);
}
