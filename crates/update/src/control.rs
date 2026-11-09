//! Bundled control tools update independently of project dependency installations.
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ControlBundle {
    pub version: String,
    pub api_version: u32,
    pub binary_sha256: String,
    pub sdk_sha256: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ControlUpdateStatus {
    pub version: String,
    pub state: String,
    pub error: Option<String>,
}
fn home() -> Result<PathBuf> {
    Ok(PathBuf::from(std::env::var_os("HOME").context("HOME is unavailable")?).join(".zuse"))
}
pub fn status() -> Result<ControlUpdateStatus> {
    let path = home()?.join("control/status.json");
    if !path.exists() {
        return Ok(ControlUpdateStatus {
            version: super::current_version().into(),
            state: "unmanaged".into(),
            error: None,
        });
    }
    Ok(serde_json::from_slice(&std::fs::read(path)?)?)
}
fn save_status(root: &Path, status: &ControlUpdateStatus) -> Result<()> {
    std::fs::create_dir_all(root.join("control"))?;
    atomic_write(
        &root.join("control/status.json"),
        &serde_json::to_vec(status)?,
    )
}
fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write;
    let mut file = tempfile::NamedTempFile::new_in(path.parent().context("missing parent")?)?;
    file.write_all(bytes)?;
    file.as_file().sync_all()?;
    file.persist(path)?;
    Ok(())
}
fn verify(path: &Path, expected: &str) -> Result<()> {
    ensure!(expected.len() == 64, "control checksum is missing");
    ensure!(
        format!("{:x}", Sha256::digest(std::fs::read(path)?)) == expected,
        "control artifact checksum mismatch"
    );
    Ok(())
}

/// Called at startup after the app has selected its version. It never changes
/// project manifests/lockfiles or installs an absent global npm package.
pub async fn synchronize_bundled() -> Result<ControlUpdateStatus> {
    let root = home()?;
    let bundle = std::env::current_exe()?
        .parent()
        .context("application directory unavailable")?
        .join("control");
    if !bundle.join("manifest.json").is_file() {
        return status();
    }
    std::fs::create_dir_all(root.join("control"))?;
    let lease = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(root.join("control/update.lock"))?;
    lease
        .try_lock()
        .context("another control update is active; retry after it finishes")?;
    let mut status = ControlUpdateStatus {
        version: super::current_version().into(),
        state: "updating".into(),
        error: None,
    };
    save_status(&root, &status)?;
    let result = async {
        let manifest: ControlBundle =
            serde_json::from_slice(&tokio::fs::read(bundle.join("manifest.json")).await?)?;
        ensure!(
            manifest.version == super::current_version() && manifest.api_version == 1,
            "incompatible bundled control tools"
        );
        let binary = install_bundle(&bundle, &root, &manifest)?;
        let probe = tokio::time::timeout(
            std::time::Duration::from_secs(30),
            tokio::process::Command::new(&binary)
                .arg("--version")
                .kill_on_drop(true)
                .output(),
        )
        .await??;
        ensure!(
            probe.status.success()
                && String::from_utf8_lossy(&probe.stdout).trim()
                    == format!("zuse-control {}", manifest.version),
            "control binary version check failed"
        );
        activate(&root, &manifest.version)?;
        update_global_sdk(
            &root.join("control").join(&manifest.version).join("sdk.tgz"),
            &manifest.version,
        )
        .await?;
        Ok::<(), anyhow::Error>(())
    }
    .await;
    match result {
        Ok(()) => status.state = "ready".into(),
        Err(error) => {
            status.state = "failed".into();
            status.error = Some(error.to_string());
        }
    }
    save_status(&root, &status)?;
    Ok(status)
}

fn binary_name() -> &'static str {
    if cfg!(windows) {
        "zuse-control.exe"
    } else {
        "zuse-control"
    }
}
fn install_bundle(bundle: &Path, root: &Path, manifest: &ControlBundle) -> Result<PathBuf> {
    ensure!(
        super::parse_version(&manifest.version).is_some(),
        "invalid control version"
    );
    ensure!(
        manifest
            .version
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-'),
        "invalid control version"
    );
    verify(&bundle.join(binary_name()), &manifest.binary_sha256)?;
    verify(&bundle.join("sdk.tgz"), &manifest.sdk_sha256)?;
    let versions = root.join("control");
    std::fs::create_dir_all(&versions)?;
    let target = versions.join(&manifest.version);
    if target.exists() {
        verify(&target.join(binary_name()), &manifest.binary_sha256)?;
        verify(&target.join("sdk.tgz"), &manifest.sdk_sha256)?;
        return Ok(target.join(binary_name()));
    }
    let stage = tempfile::tempdir_in(&versions)?;
    std::fs::copy(bundle.join(binary_name()), stage.path().join(binary_name()))?;
    std::fs::copy(bundle.join("sdk.tgz"), stage.path().join("sdk.tgz"))?;
    std::fs::write(
        stage.path().join("manifest.json"),
        serde_json::to_vec(manifest)?,
    )?;
    std::fs::rename(stage.path(), &target)?;
    Ok(target.join(binary_name()))
}
fn activate(root: &Path, version: &str) -> Result<()> {
    #[cfg(unix)]
    {
        let versions = root.join("control");
        let bin = root.join("bin");
        std::fs::create_dir_all(&bin)?;
        let current = versions.join("current");
        if let Ok(previous) = std::fs::read_link(&current) {
            if previous != PathBuf::from(version) {
                replace_link(&versions.join("previous"), &previous)?;
            }
        }
        replace_link(&current, Path::new(version))?;
        replace_link(
            &bin.join("zuse-control"),
            &versions.join("current/zuse-control"),
        )?;
        Ok(())
    }
    #[cfg(not(unix))]
    {
        let _ = (root, version);
        anyhow::bail!("managed control activation is not available on this platform")
    }
}
#[cfg(unix)]
fn replace_link(path: &Path, target: &Path) -> Result<()> {
    let stage = tempfile::tempdir_in(path.parent().context("missing link parent")?)?;
    let link = stage.path().join("link");
    std::os::unix::fs::symlink(target, &link)?;
    std::fs::rename(link, path)?;
    Ok(())
}
async fn update_global_sdk(tarball: &Path, version: &str) -> Result<()> {
    let output = match tokio::time::timeout(
        std::time::Duration::from_secs(30),
        tokio::process::Command::new("npm")
            .args(["root", "--global"])
            .kill_on_drop(true)
            .output(),
    )
    .await
    {
        Ok(Ok(output)) if output.status.success() => output,
        _ => return Ok(()),
    };
    let root = PathBuf::from(String::from_utf8(output.stdout)?.trim());
    let package = root.join("@zuse/sdk/package.json");
    if !package.is_file() {
        return Ok(());
    }
    let installed: serde_json::Value = serde_json::from_slice(&std::fs::read(&package)?)?;
    if installed.get("version").and_then(|v| v.as_str()) == Some(version) {
        return Ok(());
    }
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(120),
        tokio::process::Command::new("npm")
            .args([
                "install",
                "--global",
                "--ignore-scripts",
                "--no-audit",
                "--no-fund",
            ])
            .arg(tarball)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .status(),
    )
    .await
    .context("global SDK update timed out")??;
    ensure!(
        result.success(),
        "global SDK update failed; retry after fixing npm installation permissions"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn bundle(path: &Path, version: &str) -> ControlBundle {
        std::fs::create_dir_all(path).unwrap();
        std::fs::write(path.join(binary_name()), b"binary").unwrap();
        std::fs::write(path.join("sdk.tgz"), b"sdk").unwrap();
        ControlBundle {
            version: version.into(),
            api_version: 1,
            binary_sha256: format!("{:x}", Sha256::digest(b"binary")),
            sdk_sha256: format!("{:x}", Sha256::digest(b"sdk")),
        }
    }
    #[test]
    fn corrupted_bundle_does_not_replace_installed_tools() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("bundle");
        let root = dir.path().join("install");
        let manifest = bundle(&source, "1.0.0");
        std::fs::write(source.join("sdk.tgz"), b"corrupt").unwrap();
        assert!(install_bundle(&source, &root, &manifest).is_err());
        assert!(!root.join("control/current").exists());
    }
    #[test]
    #[cfg(unix)]
    fn update_retains_previous_and_does_not_touch_project_dependencies() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("bundle");
        let root = dir.path().join("install");
        let project = dir.path().join("project");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::write(project.join("package-lock.json"), "unchanged").unwrap();
        for version in ["1.0.0", "1.1.0"] {
            let manifest = bundle(&source, version);
            install_bundle(&source, &root, &manifest).unwrap();
            activate(&root, version).unwrap();
        }
        assert_eq!(
            std::fs::read_link(root.join("control/previous")).unwrap(),
            PathBuf::from("1.0.0")
        );
        assert_eq!(
            std::fs::read_to_string(project.join("package-lock.json")).unwrap(),
            "unchanged"
        );
        assert!(root.join("bin/zuse-control").is_file());
    }
}
