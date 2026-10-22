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
        };
        assert!(prepare(&data, &checkout, "workspace", "storage", &plan).is_err());
        assert_eq!(git(&checkout, &["rev-parse", "occupied"]).unwrap(), head);
        assert_eq!(
            git(&checkout, &["branch", "--show-current"]).unwrap(),
            "main"
        );
    }
}
