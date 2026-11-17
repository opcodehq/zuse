//! Updates for per-user Linux packages. Activation goes through the same
//! installer as the initial install, preserving launcher ownership checks.
use super::{InstallKind, Manifest};
use anyhow::{Context, ensure};
use std::path::{Path, PathBuf};

pub(super) fn detect(exe: &Path, home: Option<&Path>) -> Option<InstallKind> {
    let package = exe.parent()?.parent()?;
    if exe != package.join("libexec/zuse") || package.parent()?.file_name()? != "packages" {
        return None;
    }
    let data_home = package.parent()?.parent()?.parent()?;
    if package.parent()? != data_home.join("zuse/packages") {
        return None;
    }
    let marker = std::fs::read_to_string(package.join(".installed-by-zuse")).ok()?;
    let bin_home = if marker.trim() == "1" {
        let bin = std::env::var_os("ZUSE_BIN_HOME")
            .map(PathBuf::from)
            .or_else(|| home.map(|home| home.join(".local/bin")))?;
        // Older installers did not record custom launcher directories. Only
        // activate through a launcher that belongs to this installation.
        if std::fs::canonicalize(bin.join("zuse")).ok()?
            != std::fs::canonicalize(package.join("bin/zuse")).ok()?
        {
            return None;
        }
        bin
    } else {
        let marker: serde_json::Value = serde_json::from_str(&marker).ok()?;
        if marker["format"].as_u64()? != 2 || Path::new(marker["data_home"].as_str()?) != data_home
        {
            return None;
        }
        PathBuf::from(marker["bin_home"].as_str()?)
    };
    if !bin_home.is_absolute() {
        return None;
    }
    Some(InstallKind::LinuxPackage {
        package: package.to_owned(),
        data_home: data_home.to_owned(),
        bin_home,
    })
}

fn python() -> anyhow::Result<&'static str> {
    for candidate in ["python3", "python3.13", "python3.12", "python3.11"] {
        if std::process::Command::new(candidate)
            .args(["-c", "import sys; sys.exit(sys.version_info < (3, 11))"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
        {
            return Ok(candidate);
        }
    }
    anyhow::bail!("Python 3.11 or newer is required to update this Linux installation")
}

// Reject archive links and paths outside the versioned root before extraction.
// This runs only after checking the release archive's mandatory SHA-256.
const EXTRACT: &str = r#"
import sys, tarfile
from pathlib import Path, PurePosixPath
archive, destination, name = sys.argv[1:]
with tarfile.open(archive) as bundle:
    seen = set()
    for member in bundle.getmembers():
        path = PurePosixPath(member.name)
        if path.is_absolute() or '..' in path.parts or '\\' in member.name or not path.parts or path.parts[0] != name or str(path) in seen:
            raise ValueError('Unsafe package path')
        if not member.isfile() and not member.isdir():
            raise ValueError('Package links and special files are not allowed')
        seen.add(str(path))
    bundle.extractall(destination)
"#;

pub(super) async fn stage(
    edge_url: &str,
    manifest: &Manifest,
    data_dir: &Path,
) -> anyhow::Result<PathBuf> {
    ensure!(
        super::parse_version(&manifest.version).is_some(),
        "invalid Linux release version"
    );
    let name = format!("zuse-{}-linux-{}", manifest.version, std::env::consts::ARCH);
    let file = format!("{name}.tar.gz");
    ensure!(
        manifest
            .files
            .get(&file)
            .and_then(|file| file.sha256.as_ref())
            .is_some(),
        "Linux update has no archive checksum"
    );
    let root = data_dir.join("updates").join(&manifest.version);
    std::fs::create_dir_all(&root)?;
    let temporary = tempfile::tempdir_in(&root)?;
    let archive = temporary.path().join(&file);
    super::download_release_file(edge_url, manifest, &file, &archive).await?;
    let output = tokio::process::Command::new(python()?)
        .args(["-c", EXTRACT])
        .arg(&archive)
        .arg(temporary.path())
        .arg(&name)
        .kill_on_drop(true)
        .output()
        .await
        .context("extracting Linux update")?;
    ensure!(
        output.status.success(),
        "extracting Linux update: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let package = temporary.path().join(&name);
    // Validate the exact packaged version before exposing a Restart action.
    let metadata: serde_json::Value =
        serde_json::from_slice(&std::fs::read(package.join("manifest.json"))?)?;
    ensure!(
        metadata["version"].as_str() == Some(&manifest.version)
            && metadata["platform"] == "linux"
            && metadata["arch"] == std::env::consts::ARCH,
        "Linux package identity does not match the offered release"
    );
    let output = tokio::process::Command::new(python()?)
        .args([
            "-c",
            include_str!("../../../scripts/install-linux.py"),
            "--verify-only",
            "--package",
        ])
        .arg(&package)
        .kill_on_drop(true)
        .output()
        .await?;
    ensure!(
        output.status.success(),
        "verifying Linux package: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    super::verify_staged_binary(&package.join("libexec/zuse"), &manifest.version).await?;
    std::fs::remove_file(archive)?;
    let _ = temporary.keep();
    Ok(package)
}

pub(super) fn apply(staged: &Path, data_home: &Path, bin_home: &Path) -> anyhow::Result<()> {
    let output = std::process::Command::new(python()?)
        .args([
            "-c",
            include_str!("../../../scripts/install-linux.py"),
            "--package",
        ])
        .arg(staged)
        .env("XDG_DATA_HOME", data_home)
        .env("ZUSE_BIN_HOME", bin_home)
        .stdin(std::process::Stdio::null())
        .output()
        .context("installing Linux update")?;
    ensure!(
        output.status.success(),
        "installing Linux update: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::fs::symlink;

    #[tokio::test]
    async fn stages_the_complete_package_and_rejects_tampered_files() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let dir = tempfile::tempdir().unwrap();
        let name = format!("zuse-9.9.9-linux-{}", std::env::consts::ARCH);
        let package = dir.path().join(&name);
        std::fs::create_dir_all(package.join("libexec/web")).unwrap();
        std::fs::create_dir_all(package.join("bin")).unwrap();
        let binary = package.join("libexec/zuse");
        std::fs::write(&binary, "#!/bin/sh\necho 'zuse 9.9.9'\n").unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::write(
            package.join("bin/zuse"),
            "#!/bin/sh\nexec \"$(dirname \"$0\")/../libexec/zuse\" \"$@\"\n",
        )
        .unwrap();
        std::fs::write(package.join("libexec/web/index.html"), "bundled assets").unwrap();
        let mut files = serde_json::Map::new();
        for relative in ["libexec/zuse", "bin/zuse", "libexec/web/index.html"] {
            files.insert(
                relative.into(),
                format!(
                    "{:x}",
                    Sha256::digest(std::fs::read(package.join(relative)).unwrap())
                )
                .into(),
            );
        }
        std::fs::write(package.join("manifest.json"), serde_json::to_vec(&serde_json::json!({"format":1,"version":"9.9.9","platform":"linux","arch":std::env::consts::ARCH,"files":files})).unwrap()).unwrap();
        for tampered in [false, true] {
            if tampered {
                std::fs::write(package.join("libexec/web/index.html"), "corrupted assets").unwrap();
            }
            let archive = dir.path().join("archive.tar.gz");
            assert!(
                std::process::Command::new("tar")
                    .env("COPYFILE_DISABLE", "1")
                    .arg("-czf")
                    .arg(&archive)
                    .arg("-C")
                    .arg(dir.path())
                    .arg(&name)
                    .status()
                    .unwrap()
                    .success()
            );
            let payload = std::fs::read(archive).unwrap();
            let manifest = Manifest {
                version: "9.9.9".into(),
                files: [(
                    format!("{name}.tar.gz"),
                    super::super::FileMeta {
                        sha256: Some(format!("{:x}", Sha256::digest(&payload))),
                    },
                )]
                .into(),
            };
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let base = format!("http://{}", listener.local_addr().unwrap());
            let server = tokio::spawn(async move {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut request = [0; 4096];
                stream.read(&mut request).await.unwrap();
                stream
                    .write_all(
                        format!(
                            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            payload.len()
                        )
                        .as_bytes(),
                    )
                    .await
                    .unwrap();
                stream.write_all(&payload).await.unwrap();
            });
            let result = stage(&base, &manifest, &dir.path().join("profile")).await;
            server.await.unwrap();
            if tampered {
                assert!(format!("{:#}", result.unwrap_err()).contains("Package checksum mismatch"));
            } else {
                let staged = result.unwrap();
                assert!(staged.join("bin/zuse").is_file());
                assert_eq!(
                    std::fs::read_to_string(staged.join("libexec/web/index.html")).unwrap(),
                    "bundled assets"
                );
            }
        }
    }

    #[tokio::test]
    async fn refuses_unverified_archives_before_downloading() {
        let dir = tempfile::tempdir().unwrap();
        let manifest = Manifest {
            version: "9.9.9".into(),
            files: Default::default(),
        };
        let error = stage("http://127.0.0.1:1", &manifest, dir.path())
            .await
            .unwrap_err();
        assert!(error.to_string().contains("no archive checksum"));
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[test]
    fn detects_installer_layout_and_preserves_custom_locations() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("custom data");
        let bin = dir.path().join("custom bin");
        let package = data.join("zuse/packages/zuse-1.2.3-linux-aarch64-digest");
        std::fs::create_dir_all(package.join("libexec")).unwrap();
        std::fs::write(
            package.join(".installed-by-zuse"),
            serde_json::to_vec(&serde_json::json!({"format":2,"data_home":data,"bin_home":bin}))
                .unwrap(),
        )
        .unwrap();
        let expected = InstallKind::LinuxPackage {
            package: package.clone(),
            data_home: data.clone(),
            bin_home: bin.clone(),
        };
        assert_eq!(detect(&package.join("libexec/zuse"), None), Some(expected));
        assert_eq!(detect(&package.join("libexec/zuse-server"), None), None);
        std::fs::remove_file(package.join(".installed-by-zuse")).unwrap();
        assert_eq!(detect(&package.join("libexec/zuse"), None), None);
    }

    #[test]
    fn detects_existing_installs_and_reads_the_activated_version() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        let data = home.join(".local/share");
        let bin = home.join(".local/bin");
        let package = data.join("zuse/packages/old");
        std::fs::create_dir_all(package.join("bin")).unwrap();
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(package.join("bin/zuse"), "launcher").unwrap();
        std::fs::write(package.join(".installed-by-zuse"), "1\n").unwrap();
        symlink(package.join("bin/zuse"), bin.join("zuse")).unwrap();
        let install = detect(&package.join("libexec/zuse"), Some(home)).unwrap();
        let new = data.join("zuse/packages/new");
        std::fs::create_dir_all(new.join("bin")).unwrap();
        std::fs::write(new.join("bin/zuse"), "new launcher").unwrap();
        std::fs::write(new.join("release.json"), r#"{"version":"1.2.4-preview.1"}"#).unwrap();
        std::fs::remove_file(bin.join("zuse")).unwrap();
        symlink(new.join("bin/zuse"), bin.join("zuse")).unwrap();
        assert_eq!(
            super::super::installed_version(&install).as_deref(),
            Some("1.2.4-preview.1")
        );
    }
}
