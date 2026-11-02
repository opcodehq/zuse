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
    pub branch: String,
    pub base_ref: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_path: Option<std::path::PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_commit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repository_url: Option<String>,
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
    if plan.cache_path.is_some() && !directory.join("rust-repository-ready.json").exists() {
        install_cached_checkout(directory, checkout, workspace, incarnation, plan)?;
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
                &format!("{}^{{commit}}", plan.base_ref),
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
        let plan = CheckoutPlan {
            branch: "cloud/new".into(),
            base_ref: "topic".into(),
            cache_path: Some(cache),
            source_commit: Some(sha),
            repository_url: Some("https://github.com/example/selected.git".into()),
        };
        prepare(&data, &checkout, "workspace", "storage", &plan).unwrap();
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
}

#[derive(Serialize, Deserialize)]
struct CacheInstall {
    version: u8,
    workspace: String,
    incarnation: String,
    destination: String,
    stage: String,
    plan: CheckoutPlan,
}
fn install_cached_checkout(
    directory: &Path,
    destination: &Path,
    workspace: &str,
    incarnation: &str,
    plan: &CheckoutPlan,
) -> Result<()> {
    let cache = plan
        .cache_path
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("runtime_repository_cache_missing"))?;
    ensure!(
        cache.is_absolute()
            && destination.is_absolute()
            && !std::fs::symlink_metadata(cache)?.file_type().is_symlink(),
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
    let parent = destination
        .parent()
        .ok_or_else(|| anyhow::anyhow!("runtime_repository_destination_invalid"))?;
    std::fs::create_dir_all(parent)?;
    std::fs::create_dir_all(directory)?;
    let receipt = directory.join("rust-repository-cache-install.json");
    let install = if receipt.exists() {
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
        ensure!(
            git(cache, &["rev-parse", "--is-bare-repository"])? == "true",
            "runtime_repository_cache_not_bare"
        );
        git(
            cache,
            &["cat-file", "-e", &format!("{expected}^{{commit}}")],
        )?;
        let stage = parent.join(format!(".rust-checkout-{}", uuid::Uuid::new_v4().simple()));
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
        let selected_ref = if git(
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
        let commit = git(
            &stage,
            &[
                "rev-parse",
                "--verify",
                "--end-of-options",
                &format!("{selected_ref}^{{commit}}"),
            ],
        )?;
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
        if selected_ref != plan.base_ref {
            git(
                &stage,
                &[
                    "update-ref",
                    &format!("refs/heads/{}", plan.base_ref),
                    &commit,
                ],
            )?;
        }
        let url = plan
            .repository_url
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("runtime_repository_url_missing"))?;
        ensure!(
            url.starts_with("https://github.com/") && !url.contains(['\n', '\r', '\0', '@']),
            "runtime_repository_url_invalid"
        );
        git(&stage, &["remote", "set-url", "origin", url])?;
        let install = CacheInstall {
            version: 1,
            workspace: workspace.into(),
            incarnation: incarnation.into(),
            destination: destination.to_string_lossy().into(),
            stage: stage.to_string_lossy().into(),
            plan: plan.clone(),
        };
        atomic_write(&receipt, &serde_json::to_vec(&install)?)?;
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
        if std::fs::symlink_metadata(destination).is_ok() {
            ensure!(
                !std::fs::symlink_metadata(destination)?
                    .file_type()
                    .is_symlink()
                    && std::fs::read_dir(destination)?.next().is_none(),
                "runtime_repository_destination_occupied"
            );
            std::fs::remove_dir(destination)?;
        }
        std::fs::rename(&stage, destination)?;
        std::fs::File::open(parent)?.sync_all()?;
    }
    ensure!(
        destination.is_dir()
            && !std::fs::symlink_metadata(destination)?
                .file_type()
                .is_symlink(),
        "runtime_repository_cache_install_missing"
    );
    git(
        destination,
        &["cat-file", "-e", &format!("{expected}^{{commit}}")],
    )?;
    Ok(())
}
