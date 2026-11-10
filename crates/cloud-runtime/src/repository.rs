//! Journal first checkout selection under the caller-held lifetime runtime data lock.
use crate::identity::atomic_write;
use anyhow::{Result, bail, ensure};
use serde::{Deserialize, Serialize};
use std::{
    path::Path,
    process::{Command, Stdio},
};
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CheckoutPlan {
    #[serde(default, skip_serializing_if = "is_false")]
    pub fetch_remote: bool,
    pub branch: String,
    pub base_ref: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_path: Option<std::path::PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_commit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repository_url: Option<String>,
}
fn is_false(value: &bool) -> bool {
    !*value
}
/// Read the journaled source, not HEAD (which an agent or user may already have advanced).
pub fn prepared_source(directory: &Path, workspace: &str, incarnation: &str) -> Result<String> {
    let journal: Journal = serde_json::from_slice(&std::fs::read(
        directory.join("rust-repository-ready.json"),
    )?)?;
    ensure!(
        journal.ready
            && journal.version == 1
            && journal.workspace == workspace
            && journal.incarnation == incarnation
            && matches!(journal.base_commit.len(), 40 | 64)
            && journal.base_commit.bytes().all(|b| b.is_ascii_hexdigit()),
        "runtime_repository_receipt_invalid"
    );
    Ok(journal.base_commit)
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Journal {
    version: u8,
    workspace: String,
    incarnation: String,
    checkout: String,
    plan: CheckoutPlan,
    base_commit: String,
    ready: bool,
}
fn git(root: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .stdin(Stdio::null())
        .output()?;
    ensure!(output.status.success(), "runtime_repository_command_failed");
    ensure!(
        output.stdout.len() <= 1024 * 1024,
        "runtime_repository_output_too_large"
    );
    Ok(String::from_utf8(output.stdout)?.trim().to_owned())
}
pub fn prepare(
    directory: &Path,
    checkout: &Path,
    workspace: &str,
    incarnation: &str,
    plan: &CheckoutPlan,
) -> Result<()> {
    prepare_with_fetch(
        directory,
        checkout,
        workspace,
        incarnation,
        plan,
        &fetch_remote,
    )
}
/// The helper contains only a runtime executable/socket path. Credentials are
/// delivered over its private pipe, never appended to this command or URL.
pub fn prepare_authenticated(
    directory: &Path,
    checkout: &Path,
    workspace: &str,
    incarnation: &str,
    plan: &CheckoutPlan,
    helper: &str,
) -> Result<()> {
    prepare_with_fetch(
        directory,
        checkout,
        workspace,
        incarnation,
        plan,
        &|root, plan| fetch_remote_with_helper(root, plan, Some(helper)),
    )
}
/// Keep the blocking checkout worker owned until any cancelled fetch has been
/// killed and reaped, so Stop cannot leave Git writing after preparation exits.
pub fn prepare_cancellable(
    directory: &Path,
    checkout: &Path,
    workspace: &str,
    incarnation: &str,
    plan: &CheckoutPlan,
    helper: Option<&str>,
    cancel: &tokio_util::sync::CancellationToken,
) -> Result<()> {
    ensure!(!cancel.is_cancelled(), "runtime_repository_fetch_cancelled");
    prepare_with_fetch(
        directory,
        checkout,
        workspace,
        incarnation,
        plan,
        &|root, plan| fetch_remote_with_helper_cancellable(root, plan, helper, Some(cancel)),
    )
}
fn prepare_with_fetch(
    directory: &Path,
    checkout: &Path,
    workspace: &str,
    incarnation: &str,
    plan: &CheckoutPlan,
    fetch: &dyn Fn(&Path, &CheckoutPlan) -> Result<String>,
) -> Result<()> {
    if (plan.cache_path.is_some() || plan.fetch_remote)
        && !directory.join("rust-repository-ready.json").exists()
    {
        install_cached_checkout(directory, checkout, workspace, incarnation, plan, fetch)?;
    }
    let checkout = std::fs::canonicalize(checkout)?;
    let root = git(&checkout, &["rev-parse", "--show-toplevel"])?;
    ensure!(
        std::fs::canonicalize(root)? == checkout,
        "runtime_repository_root_mismatch"
    );
    let marker = directory.join("rust-repository-ready.json");
    let prior = match std::fs::read(&marker) {
        Ok(bytes) => Some(serde_json::from_slice::<Journal>(&bytes)?),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error.into()),
    };
    let mut journal = if let Some(prior) = prior {
        ensure!(
            prior.version == 1
                && prior.workspace == workspace
                && prior.incarnation == incarnation
                && prior.checkout == checkout.to_string_lossy()
                && prior.plan == *plan,
            "runtime_repository_identity_mismatch"
        );
        if prior.ready {
            return Ok(());
        }
        prior
    } else {
        git(&checkout, &["check-ref-format", "--branch", &plan.branch])?;
        ensure!(
            !plan.base_ref.starts_with('-') && !plan.base_ref.contains('\0'),
            "runtime_repository_ref_invalid"
        );
        let base = git(
            &checkout,
            &[
                "rev-parse",
                "--verify",
                "--end-of-options",
                &format!(
                    "{}^{{commit}}",
                    if plan.fetch_remote {
                        "refs/zuse/requested-source"
                    } else {
                        &plan.base_ref
                    }
                ),
            ],
        )?;
        let journal = Journal {
            version: 1,
            workspace: workspace.into(),
            incarnation: incarnation.into(),
            checkout: checkout.to_string_lossy().into(),
            plan: plan.clone(),
            base_commit: base,
            ready: false,
        };
        atomic_write(&marker, &serde_json::to_vec(&journal)?)?;
        journal
    };
    ensure!(
        git(
            &checkout,
            &["status", "--porcelain", "--untracked-files=normal"]
        )?
        .is_empty(),
        "runtime_repository_checkout_not_clean"
    );
    let reference = format!("refs/heads/{}", plan.branch);
    let existing = Command::new("git")
        .arg("-C")
        .arg(&checkout)
        .args(["rev-parse", "--verify", "--quiet", &reference])
        .stdin(Stdio::null())
        .output()?;
    if existing.status.success() {
        ensure!(
            String::from_utf8(existing.stdout)?.trim() == journal.base_commit,
            "runtime_repository_branch_occupied"
        );
        git(&checkout, &["switch", "--", &plan.branch])?;
    } else if existing.status.code() == Some(1) {
        git(
            &checkout,
            &["switch", "-c", &plan.branch, &journal.base_commit],
        )?;
    } else {
        bail!("runtime_repository_command_failed");
    }
    journal.ready = true;
    atomic_write(&marker, &serde_json::to_vec(&journal)?)?;
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    #[test]
    fn cancellation_kills_and_reaps_a_stopped_fetch_process() {
        use std::os::unix::process::CommandExt;
        let mut command = Command::new("sh");
        command.args(["-c", "kill -STOP $$; sleep 30"]);
        command.process_group(0);
        let child = command.spawn().unwrap();
        let pid = child.id();
        let cancel = tokio_util::sync::CancellationToken::new();
        let signal = cancel.clone();
        let trigger = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(40));
            signal.cancel();
        });
        let before = std::time::Instant::now();
        let result = wait_for_fetch(child, Some(&cancel), std::time::Duration::from_secs(1));
        trigger.join().unwrap();
        assert_eq!(
            result.unwrap_err().to_string(),
            "runtime_repository_fetch_cancelled"
        );
        assert!(before.elapsed() < std::time::Duration::from_millis(500));
        assert_eq!(
            unsafe { libc::kill(pid as i32, 0) },
            -1,
            "fetch must be reaped before returning"
        );
    }
    fn repository() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
        let root = tempfile::tempdir().unwrap();
        let checkout = root.path().join("checkout");
        let data = root.path().join("data");
        std::fs::create_dir(&checkout).unwrap();
        std::fs::create_dir(&data).unwrap();
        git(&checkout, &["init", "--initial-branch=main"]).unwrap();
        git(
            &checkout,
            &["config", "user.email", "fixture@example.invalid"],
        )
        .unwrap();
        git(&checkout, &["config", "user.name", "Fixture"]).unwrap();
        std::fs::write(checkout.join("tracked"), "original").unwrap();
        git(&checkout, &["add", "tracked"]).unwrap();
        git(&checkout, &["commit", "-m", "Original"]).unwrap();
        (root, checkout, data)
    }
    #[test]
    fn reopening_preserves_advanced_branch_and_changed_bytes() {
        let (_root, checkout, data) = repository();
        let _lock = crate::DataLock::acquire(&data, None).unwrap();
        let plan = CheckoutPlan {
            fetch_remote: false,
            branch: "cloud/feature".into(),
            base_ref: "main".into(),
            cache_path: None,
            source_commit: None,
            repository_url: None,
        };
        prepare(&data, &checkout, "workspace", "storage", &plan).unwrap();
        std::fs::write(checkout.join("tracked"), "advanced").unwrap();
        git(&checkout, &["commit", "-am", "Advanced"]).unwrap();
        let head = git(&checkout, &["rev-parse", "HEAD"]).unwrap();
        std::fs::write(checkout.join("tracked"), "working changes").unwrap();
        std::fs::write(checkout.join("untracked"), "new bytes").unwrap();
        prepare(&data, &checkout, "workspace", "storage", &plan).unwrap();
        assert_eq!(git(&checkout, &["rev-parse", "HEAD"]).unwrap(), head);
        assert_eq!(
            std::fs::read_to_string(checkout.join("tracked")).unwrap(),
            "working changes"
        );
        assert!(checkout.join("untracked").exists());
        assert!(prepare(&data, &checkout, "different-workspace", "storage", &plan).is_err());
    }
    #[test]
    fn cached_repository_selection_and_reopen_preserve_user_work() {
        let (root, original, data) = repository();
        git(&original, &["branch", "topic"]).unwrap();
        let cache = root.path().join("selected.git");
        let output = Command::new("git")
            .arg("clone")
            .arg("--bare")
            .arg(&original)
            .arg(&cache)
            .output()
            .unwrap();
        assert!(output.status.success());
        let sha = git(&original, &["rev-parse", "HEAD"]).unwrap();
        let checkout = root.path().join("new-workspace");
        std::fs::create_dir(&checkout).unwrap();
        let directory_handle = std::fs::File::open(&checkout).unwrap();
        let plan = CheckoutPlan {
            fetch_remote: false,
            branch: "cloud/new".into(),
            base_ref: "topic".into(),
            cache_path: Some(cache),
            source_commit: Some(sha),
            repository_url: Some("https://github.com/example/selected.git".into()),
        };
        prepare(&data, &checkout, "workspace", "storage", &plan).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            assert_eq!(
                directory_handle.metadata().unwrap().ino(),
                std::fs::metadata(&checkout).unwrap().ino(),
                "an already-started harness must retain its working directory"
            );
        }
        assert_eq!(
            std::fs::read_to_string(checkout.join("tracked")).unwrap(),
            "original"
        );
        git(
            &checkout,
            &["config", "user.email", "fixture@example.invalid"],
        )
        .unwrap();
        git(&checkout, &["config", "user.name", "Fixture"]).unwrap();
        std::fs::write(checkout.join("tracked"), "advanced").unwrap();
        git(&checkout, &["commit", "-am", "Advanced"]).unwrap();
        let head = git(&checkout, &["rev-parse", "HEAD"]).unwrap();
        std::fs::write(checkout.join("tracked"), "working changes").unwrap();
        std::fs::write(checkout.join("untracked"), "user bytes").unwrap();
        prepare(&data, &checkout, "workspace", "storage", &plan).unwrap();
        assert_eq!(git(&checkout, &["rev-parse", "HEAD"]).unwrap(), head);
        assert_eq!(
            std::fs::read_to_string(checkout.join("tracked")).unwrap(),
            "working changes"
        );
        assert_eq!(
            std::fs::read_to_string(checkout.join("untracked")).unwrap(),
            "user bytes"
        );
        assert_eq!(
            git(&checkout, &["remote", "get-url", "origin"]).unwrap(),
            "https://github.com/example/selected.git"
        );
        assert_eq!(
            std::fs::read_to_string(original.join("tracked")).unwrap(),
            "original"
        );
        // A second runtime cannot install over occupied user files.
        #[cfg(unix)]
        {
            let occupied = root.path().join("occupied-link");
            std::os::unix::fs::symlink(root.path().join("absent-user-target"), &occupied).unwrap();
            assert!(
                prepare(
                    &root.path().join("link-data"),
                    &occupied,
                    "link-workspace",
                    "storage",
                    &plan
                )
                .is_err()
            );
            assert!(
                std::fs::symlink_metadata(&occupied)
                    .unwrap()
                    .file_type()
                    .is_symlink()
            );
        }
        let other_data = root.path().join("other-data");
        assert!(prepare(&other_data, &checkout, "other-workspace", "storage", &plan).is_err());
        assert_eq!(git(&checkout, &["rev-parse", "HEAD"]).unwrap(), head);
    }
    #[test]
    fn conflicting_branch_is_not_reset() {
        let (_root, checkout, data) = repository();
        let _lock = crate::DataLock::acquire(&data, None).unwrap();
        git(&checkout, &["switch", "-c", "occupied"]).unwrap();
        std::fs::write(checkout.join("tracked"), "newer").unwrap();
        git(&checkout, &["commit", "-am", "Newer"]).unwrap();
        let head = git(&checkout, &["rev-parse", "HEAD"]).unwrap();
        git(&checkout, &["switch", "main"]).unwrap();
        let plan = CheckoutPlan {
            fetch_remote: false,
            branch: "occupied".into(),
            base_ref: "main".into(),
            cache_path: None,
            source_commit: None,
            repository_url: None,
        };
        assert!(prepare(&data, &checkout, "workspace", "storage", &plan).is_err());
        assert_eq!(git(&checkout, &["rev-parse", "occupied"]).unwrap(), head);
        assert_eq!(
            git(&checkout, &["branch", "--show-current"]).unwrap(),
            "main"
        );
    }

    #[test]
    fn interrupted_staged_install_resumes_moves_without_replacing_the_open_directory() {
        let (root, source, data) = repository();
        std::fs::create_dir_all(&data).unwrap();
        let stage = root.path().join(".rust-checkout-interrupted");
        assert!(
            Command::new("git")
                .args(["clone", "--no-hardlinks"])
                .arg(&source)
                .arg(&stage)
                .output()
                .unwrap()
                .status
                .success()
        );
        let destination = root.path().join("early-agent-cwd");
        std::fs::create_dir(&destination).unwrap();
        let open_directory = std::fs::File::open(&destination).unwrap();
        let entries: Vec<_> = std::fs::read_dir(&stage)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        let plan = CheckoutPlan {
            fetch_remote: true,
            branch: "task/early".into(),
            base_ref: "main".into(),
            cache_path: None,
            source_commit: None,
            repository_url: None,
        };
        let install = CacheInstall {
            version: 1,
            workspace: "workspace".into(),
            incarnation: "disk".into(),
            destination: destination.to_string_lossy().into(),
            stage: stage.to_string_lossy().into(),
            plan: plan.clone(),
            entries: entries.clone(),
        };
        atomic_write(
            &data.join("rust-repository-cache-install.json"),
            &serde_json::to_vec(&install).unwrap(),
        )
        .unwrap();
        // Simulate loss of the process after its first journaled move.
        std::fs::rename(stage.join(&entries[0]), destination.join(&entries[0])).unwrap();
        install_cached_checkout(&data, &destination, "workspace", "disk", &plan, &|_, _| {
            panic!("a retained install must not fetch again")
        })
        .unwrap();
        assert!(!stage.exists());
        assert!(destination.join("tracked").is_file());
        assert!(destination.join(".git").is_dir());
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            assert_eq!(
                open_directory.metadata().unwrap().ino(),
                std::fs::metadata(&destination).unwrap().ino()
            );
        }
    }

    #[test]
    fn new_tasks_fetch_the_requested_current_commit_with_or_without_a_cache_and_resume_never_fetches()
     {
        for cached in [true, false] {
            for source in ["branch", "full-branch", "tag", "sha", "head"] {
                let (root, remote, data) = repository();
                let old = git(&remote, &["rev-parse", "HEAD"]).unwrap();
                let cache = root.path().join("cache.git");
                if cached {
                    assert!(
                        Command::new("git")
                            .arg("clone")
                            .arg("--bare")
                            .arg(&remote)
                            .arg(&cache)
                            .output()
                            .unwrap()
                            .status
                            .success()
                    );
                }
                std::fs::write(remote.join("tracked"), "current remote").unwrap();
                git(&remote, &["commit", "-am", "Current remote"]).unwrap();
                let current = git(&remote, &["rev-parse", "HEAD"]).unwrap();
                git(
                    &remote,
                    &["tag", "-a", "release-v1", "-m", "Release fixture"],
                )
                .unwrap();
                let checkout = root.path().join("task-checkout");
                std::fs::create_dir(&checkout).unwrap();
                let plan = CheckoutPlan {
                    fetch_remote: true,
                    branch: "cloud/task".into(),
                    base_ref: match source {
                        "full-branch" => "refs/heads/main".into(),
                        "tag" => "refs/tags/release-v1".into(),
                        "sha" => current.clone(),
                        "head" => "HEAD".into(),
                        _ => "main".into(),
                    },
                    cache_path: cached.then_some(cache),
                    source_commit: cached.then_some(old),
                    repository_url: Some("https://github.com/example/app.git".into()),
                };
                let calls = std::cell::Cell::new(0);
                let fetch = |stage: &Path, requested: &CheckoutPlan| {
                    calls.set(calls.get() + 1);
                    // A real local Git remote substitutes for GitHub transport only;
                    // clone, checkout, branch admission and receipts are production code.
                    git(
                        stage,
                        &[
                            "fetch",
                            "--quiet",
                            remote.to_str().unwrap(),
                            &requested.base_ref,
                        ],
                    )?;
                    git(stage, &["rev-parse", "FETCH_HEAD^{commit}"])
                };
                prepare_with_fetch(&data, &checkout, "workspace", "storage", &plan, &fetch)
                    .unwrap();
                assert_eq!(git(&checkout, &["rev-parse", "HEAD"]).unwrap(), current);
                assert_eq!(
                    prepared_source(&data, "workspace", "storage").unwrap(),
                    current
                );
                assert!(prepared_source(&data, "another-workspace", "storage").is_err());
                assert_eq!(
                    git(&checkout, &["branch", "--show-current"]).unwrap(),
                    "cloud/task"
                );
                assert_eq!(
                    std::fs::read_to_string(checkout.join("tracked")).unwrap(),
                    "current remote"
                );
                if source == "tag" {
                    assert_eq!(
                        git(&checkout, &["cat-file", "-t", "refs/tags/release-v1"]).unwrap(),
                        "tag"
                    );
                }
                std::fs::write(checkout.join("tracked"), "uncommitted user work").unwrap();
                std::fs::write(checkout.join("untracked"), "new user file").unwrap();
                std::fs::write(remote.join("tracked"), "later remote").unwrap();
                git(&remote, &["commit", "-am", "Later remote"]).unwrap();
                prepare_with_fetch(&data, &checkout, "workspace", "storage", &plan, &fetch)
                    .unwrap();
                assert_eq!(calls.get(), 1);
                assert_eq!(git(&checkout, &["rev-parse", "HEAD"]).unwrap(), current);
                assert_eq!(
                    std::fs::read_to_string(checkout.join("tracked")).unwrap(),
                    "uncommitted user work"
                );
                assert_eq!(
                    std::fs::read_to_string(checkout.join("untracked")).unwrap(),
                    "new user file"
                );
            }
        }
    }

    #[test]
    fn public_fetch_rejects_credential_and_foreign_repository_urls() {
        assert!(valid_repository_url("https://github.com/example/app.git"));
        for url in [
            "http://github.com/example/app.git",
            "https://github.com.attacker.test/example/app.git",
            "https://user:secret@github.com/example/app.git",
            "https://github.com/example/app.git?token=secret",
            "https://github.com/example/app.git#secret",
            "file:///private/repository",
        ] {
            assert!(!valid_repository_url(url));
        }
    }
}

#[derive(Serialize, Deserialize)]
struct CacheInstall {
    version: u8,
    workspace: String,
    incarnation: String,
    destination: String,
    stage: String,
    plan: CheckoutPlan,
    #[serde(default)]
    entries: Vec<std::ffi::OsString>,
}
fn install_cached_checkout(
    directory: &Path,
    destination: &Path,
    workspace: &str,
    incarnation: &str,
    plan: &CheckoutPlan,
    fetch: &dyn Fn(&Path, &CheckoutPlan) -> Result<String>,
) -> Result<()> {
    let cache = plan.cache_path.as_deref();
    ensure!(
        destination.is_absolute(),
        "runtime_repository_destination_invalid"
    );
    let expected = if let Some(cache) = cache {
        ensure!(
            cache.is_absolute() && !std::fs::symlink_metadata(cache)?.file_type().is_symlink(),
            "runtime_repository_cache_invalid"
        );
        let expected = plan
            .source_commit
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("runtime_repository_cache_commit_missing"))?;
        ensure!(
            (expected.len() == 40 || expected.len() == 64)
                && expected.bytes().all(|b| b.is_ascii_hexdigit()),
            "runtime_repository_cache_commit_invalid"
        );
        Some(expected)
    } else {
        ensure!(plan.fetch_remote, "runtime_repository_cache_missing");
        None
    };
    let parent = destination
        .parent()
        .ok_or_else(|| anyhow::anyhow!("runtime_repository_destination_invalid"))?;
    std::fs::create_dir_all(parent)?;
    std::fs::create_dir_all(directory)?;
    let receipt = directory.join("rust-repository-cache-install.json");
    let mut install = if receipt.exists() {
        let prior: CacheInstall = serde_json::from_slice(&std::fs::read(&receipt)?)?;
        ensure!(
            prior.version == 1
                && prior.workspace == workspace
                && prior.incarnation == incarnation
                && prior.destination == destination.to_string_lossy()
                && prior.plan == *plan,
            "runtime_repository_cache_identity_mismatch"
        );
        prior
    } else {
        if std::fs::symlink_metadata(destination).is_ok() {
            ensure!(
                !std::fs::symlink_metadata(destination)?
                    .file_type()
                    .is_symlink()
                    && std::fs::read_dir(destination)?.next().is_none(),
                "runtime_repository_destination_occupied"
            );
        }
        let stage = parent.join(format!(".rust-checkout-{}", uuid::Uuid::new_v4().simple()));
        let mut cleanup = StagingCheckout {
            path: stage.clone(),
            preserve: false,
        };
        if let Some(cache) = cache {
            ensure!(
                git(cache, &["rev-parse", "--is-bare-repository"])? == "true",
                "runtime_repository_cache_not_bare"
            );
            git(
                cache,
                &[
                    "cat-file",
                    "-e",
                    &format!("{}^{{commit}}", expected.expect("cache requires commit")),
                ],
            )?;
            let cloned = Command::new("git")
                .args([
                    "-c",
                    "core.hooksPath=/dev/null",
                    "clone",
                    "--quiet",
                    "--no-checkout",
                    "--no-hardlinks",
                    "--",
                ])
                .arg(cache)
                .arg(&stage)
                .stdin(Stdio::null())
                .output()?;
            ensure!(
                cloned.status.success(),
                "runtime_repository_cache_clone_failed"
            );
        } else {
            std::fs::create_dir(&stage)?;
            git(
                &stage,
                &["init", "--quiet", "--initial-branch=zuse-preparing"],
            )?;
            git(
                &stage,
                &[
                    "remote",
                    "add",
                    "origin",
                    "https://github.com/placeholder/placeholder.git",
                ],
            )?;
        }
        let url = plan
            .repository_url
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("runtime_repository_url_missing"))?;
        ensure!(valid_repository_url(url), "runtime_repository_url_invalid");
        git(&stage, &["remote", "set-url", "origin", url])?;
        let selected_ref = if plan.fetch_remote {
            "FETCH_HEAD".to_owned()
        } else if git(
            &stage,
            &[
                "rev-parse",
                "--verify",
                "--end-of-options",
                &format!("{}^{{commit}}", plan.base_ref),
            ],
        )
        .is_ok()
        {
            plan.base_ref.clone()
        } else {
            format!("refs/remotes/origin/{}", plan.base_ref)
        };
        let commit = if plan.fetch_remote {
            fetch(&stage, plan)?
        } else {
            git(
                &stage,
                &[
                    "rev-parse",
                    "--verify",
                    "--end-of-options",
                    &format!("{selected_ref}^{{commit}}"),
                ],
            )?
        };
        ensure!(
            matches!(commit.len(), 40 | 64) && commit.bytes().all(|b| b.is_ascii_hexdigit()),
            "runtime_repository_remote_commit_invalid"
        );
        // This is a new private staging checkout; no user tree is reset or replaced.
        git(
            &stage,
            &[
                "-c",
                "core.hooksPath=/dev/null",
                "checkout",
                "--quiet",
                "--detach",
                &commit,
            ],
        )?;
        if plan.fetch_remote && plan.base_ref.starts_with("refs/") {
            git(&stage, &["update-ref", &plan.base_ref, "FETCH_HEAD"])?;
        } else if selected_ref != plan.base_ref
            && plan.base_ref != "HEAD"
            && !plan.base_ref.starts_with("refs/")
            && !(matches!(plan.base_ref.len(), 40 | 64)
                && plan.base_ref.bytes().all(|b| b.is_ascii_hexdigit()))
        {
            git(
                &stage,
                &[
                    "update-ref",
                    &format!("refs/heads/{}", plan.base_ref),
                    &commit,
                ],
            )?;
            if plan.fetch_remote {
                git(
                    &stage,
                    &[
                        "update-ref",
                        &format!("refs/remotes/origin/{}", plan.base_ref),
                        &commit,
                    ],
                )?;
            }
        }
        // The branch resolver uses the fetched commit even for a full ref or SHA.
        if plan.fetch_remote {
            git(
                &stage,
                &["update-ref", "refs/zuse/requested-source", &commit],
            )?;
        }
        let install = CacheInstall {
            version: 1,
            workspace: workspace.into(),
            incarnation: incarnation.into(),
            destination: destination.to_string_lossy().into(),
            stage: stage.to_string_lossy().into(),
            plan: plan.clone(),
            entries: vec![],
        };
        atomic_write(&receipt, &serde_json::to_vec(&install)?)?;
        cleanup.preserve = true;
        install
    };
    let stage = std::path::PathBuf::from(&install.stage);
    ensure!(
        stage.parent() == Some(parent)
            && stage
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with(".rust-checkout-")),
        "runtime_repository_cache_stage_invalid"
    );
    if stage.exists() {
        // Preserve the inode held by a concurrently started harness/PTY. The
        // complete staged tree is installed under a durable list of entries;
        // interruption resumes moves, never overwrites an existing entry.
        if install.entries.is_empty() {
            ensure!(
                !destination.exists() || std::fs::read_dir(destination)?.next().is_none(),
                "runtime_repository_destination_occupied"
            );
            install.entries = std::fs::read_dir(&stage)?
                .map(|entry| entry.map(|entry| entry.file_name()))
                .collect::<std::io::Result<_>>()?;
            atomic_write(&receipt, &serde_json::to_vec(&install)?)?;
        }
        std::fs::create_dir_all(destination)?;
        ensure!(
            !std::fs::symlink_metadata(destination)?
                .file_type()
                .is_symlink(),
            "runtime_repository_destination_invalid"
        );
        for entry in std::fs::read_dir(destination)? {
            ensure!(
                install.entries.contains(&entry?.file_name()),
                "runtime_repository_destination_occupied"
            );
        }
        for entry in &install.entries {
            let path = std::path::Path::new(entry);
            ensure!(
                path.components().count() == 1
                    && matches!(
                        path.components().next(),
                        Some(std::path::Component::Normal(_))
                    ),
                "runtime_repository_cache_identity_mismatch"
            );
            let from = stage.join(entry);
            let to = destination.join(entry);
            match (
                std::fs::symlink_metadata(&from),
                std::fs::symlink_metadata(&to),
            ) {
                (Ok(_), Err(error)) if error.kind() == std::io::ErrorKind::NotFound => {
                    std::fs::rename(from, to)?
                }
                (Err(error), Ok(_)) if error.kind() == std::io::ErrorKind::NotFound => {}
                _ => bail!("runtime_repository_install_conflict"),
            }
        }
        std::fs::File::open(destination)?.sync_all()?;
        std::fs::File::open(&stage)?.sync_all()?;
        std::fs::remove_dir(&stage)?;
        std::fs::File::open(parent)?.sync_all()?;
    }
    ensure!(
        destination.is_dir()
            && !std::fs::symlink_metadata(destination)?
                .file_type()
                .is_symlink(),
        "runtime_repository_cache_install_missing"
    );
    if let Some(expected) = expected {
        git(
            destination,
            &["cat-file", "-e", &format!("{expected}^{{commit}}")],
        )?;
    }
    Ok(())
}

struct StagingCheckout {
    path: std::path::PathBuf,
    preserve: bool,
}
impl Drop for StagingCheckout {
    fn drop(&mut self) {
        if !self.preserve {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }
}
fn valid_repository_url(value: &str) -> bool {
    reqwest::Url::parse(value).ok().is_some_and(|url| {
        url.scheme() == "https"
            && url.host_str() == Some("github.com")
            && url.username().is_empty()
            && url.password().is_none()
            && url.query().is_none()
            && url.fragment().is_none()
            && url.port().is_none()
            && !url.path().contains(['\n', '\r', '\0'])
            && url.path().trim_matches('/').split('/').count() == 2
    })
}
fn fetch_remote(checkout: &Path, plan: &CheckoutPlan) -> Result<String> {
    fetch_remote_with_helper(checkout, plan, None)
}
fn fetch_remote_with_helper(
    checkout: &Path,
    plan: &CheckoutPlan,
    helper: Option<&str>,
) -> Result<String> {
    fetch_remote_with_helper_cancellable(checkout, plan, helper, None)
}
fn fetch_remote_with_helper_cancellable(
    checkout: &Path,
    plan: &CheckoutPlan,
    helper: Option<&str>,
    cancel: Option<&tokio_util::sync::CancellationToken>,
) -> Result<String> {
    ensure!(
        !cancel.is_some_and(|cancel| cancel.is_cancelled()),
        "runtime_repository_fetch_cancelled"
    );
    let url = plan
        .repository_url
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("runtime_repository_url_missing"))?;
    ensure!(
        valid_repository_url(url)
            && !plan.base_ref.starts_with('-')
            && !plan.base_ref.contains('\0'),
        "runtime_repository_ref_invalid"
    );
    if let Some(helper) = helper {
        ensure!(
            helper.len() <= 4096 && !helper.contains(['\n', '\r', '\0']),
            "runtime_git_helper_invalid"
        );
        git(
            checkout,
            &[
                "config",
                "--local",
                "--replace-all",
                "credential.helper",
                "",
            ],
        )?;
        git(
            checkout,
            &["config", "--local", "--add", "credential.helper", helper],
        )?;
        git(
            checkout,
            &["config", "--local", "credential.useHttpPath", "true"],
        )?;
    }
    let mut command = Command::new("git");
    command.arg("-C").arg(checkout).args([
        "-c",
        "credential.helper=",
        "-c",
        "http.extraHeader=",
        "-c",
        "http.followRedirects=false",
        "-c",
        "core.hooksPath=/dev/null",
    ]);
    if let Some(helper) = helper {
        command.args([
            "-c",
            &format!("credential.helper={helper}"),
            "-c",
            "credential.useHttpPath=true",
        ]);
    }
    command
        .args([
            "fetch",
            "--quiet",
            "--no-tags",
            "--depth=1",
            "--filter=blob:none",
            "--",
            "origin",
            &plan.base_ref,
        ])
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_ASKPASS", "/bin/false")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_COUNT", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let child = command.spawn()?;
    let pid = child.id();
    tracing::info!(
        pid,
        started_at_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64,
        "Cloud Git fetch process started"
    );
    wait_for_fetch(child, cancel, std::time::Duration::from_secs(120))?;
    git(checkout, &["rev-parse", "--verify", "FETCH_HEAD^{commit}"])
}

fn wait_for_fetch(
    mut child: std::process::Child,
    cancel: Option<&tokio_util::sync::CancellationToken>,
    timeout: std::time::Duration,
) -> Result<()> {
    let pid = child.id();
    let deadline = std::time::Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait()? {
            ensure!(status.success(), "runtime_repository_fetch_failed");
            break;
        }
        let cancelled = cancel.is_some_and(|cancel| cancel.is_cancelled());
        if cancelled || std::time::Instant::now() >= deadline {
            #[cfg(unix)]
            unsafe {
                libc::kill(-(pid as i32), libc::SIGKILL);
            }
            let _ = child.kill();
            let _ = child.wait();
            anyhow::bail!(if cancelled {
                "runtime_repository_fetch_cancelled"
            } else {
                "runtime_repository_fetch_timeout"
            });
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    Ok(())
}
