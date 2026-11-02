//! Fixed release artifacts, verified before execution. No PATH fallback or updater.
use super::super::{Error, Result};
use futures::StreamExt;
use sha2::{Digest, Sha256};
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::io::AsyncReadExt;

pub(super) const VERSION: &str = "2026.7.2";
const MAX_BYTES: u64 = 128 * 1024 * 1024;
struct Asset {
    name: &'static str,
    sha256: &'static str,
    archive: bool,
}
fn asset(os: &str, arch: &str) -> Result<Asset> {
    let (name, sha256, archive) = match (os, arch) {
        ("linux", "x86_64") => (
            "cloudflared-linux-amd64",
            "ec905ea7b7e327ff8abdde8cb64697a2152de74dbcdbf6aec9db8364eb3886cd",
            false,
        ),
        ("linux", "aarch64") => (
            "cloudflared-linux-arm64",
            "405df476437e027fc6d18729a5a77155c0a33a6082aeee60a799a688f3052e66",
            false,
        ),
        ("macos", "x86_64") => (
            "cloudflared-darwin-amd64.tgz",
            "a5afb0ba3da859da47bebc9a918d5b196bf7e4aec23589419b46356731bcc75f",
            true,
        ),
        ("macos", "aarch64") => (
            "cloudflared-darwin-arm64.tgz",
            "0588df58494a6cadd38b9deb6078908a5054063c80784d92fdb8d4a5f3de1c67",
            true,
        ),
        _ => return Err(Error::Unavailable),
    };
    Ok(Asset {
        name,
        sha256,
        archive,
    })
}
fn verified(path: &Path, expected: &str) -> bool {
    let Ok(metadata) = std::fs::symlink_metadata(path) else {
        return false;
    };
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > MAX_BYTES {
        return false;
    }
    let Ok(mut file) = std::fs::File::open(path) else {
        return false;
    };
    let mut hash = Sha256::new();
    let mut bytes = [0u8; 65536];
    let mut count = 0;
    loop {
        let Ok(n) = file.read(&mut bytes) else {
            return false;
        };
        if n == 0 {
            break;
        }
        count += n as u64;
        if count > MAX_BYTES {
            return false;
        }
        hash.update(&bytes[..n]);
    }
    format!("{:x}", hash.finalize()) == expected
}
fn executable(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
            .map_err(|_| Error::Storage)?;
    }
    Ok(())
}
async fn download(directory: &Path, asset: &Asset, target: &Path) -> Result<()> {
    let http = reqwest::Client::builder()
        .https_only(true)
        .redirect(reqwest::redirect::Policy::limited(5))
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(120))
        .build()
        .map_err(|_| Error::Unavailable)?;
    let response = http
        .get(format!(
            "https://github.com/cloudflare/cloudflared/releases/download/{VERSION}/{}",
            asset.name
        ))
        .send()
        .await
        .map_err(|_| Error::Unavailable)?;
    if !response.status().is_success() || response.content_length().is_some_and(|n| n > MAX_BYTES) {
        return Err(Error::Unavailable);
    }
    let mut file = tempfile::NamedTempFile::new_in(directory).map_err(|_| Error::Storage)?;
    let mut hash = Sha256::new();
    let mut size = 0;
    let mut chunks = response.bytes_stream();
    while let Some(chunk) = chunks.next().await {
        let chunk = chunk.map_err(|_| Error::Unavailable)?;
        size += chunk.len() as u64;
        if size > MAX_BYTES {
            return Err(Error::Invalid);
        }
        hash.update(&chunk);
        file.write_all(&chunk).map_err(|_| Error::Storage)?;
    }
    if size == 0 || format!("{:x}", hash.finalize()) != asset.sha256 {
        return Err(Error::Invalid);
    }
    file.as_file().sync_all().map_err(|_| Error::Storage)?;
    file.persist(target).map_err(|_| Error::Storage)?;
    Ok(())
}
pub(super) async fn ensure(root: &Path) -> Result<PathBuf> {
    let asset = asset(std::env::consts::OS, std::env::consts::ARCH)?;
    let directory = root.join("runtime/cloudflared").join(VERSION);
    std::fs::create_dir_all(&directory).map_err(|_| Error::Storage)?;
    executable(&directory)?;
    let target = directory.join("cloudflared");
    let artifact = if asset.archive {
        directory.join(asset.name)
    } else {
        target.clone()
    };
    if !verified(&artifact, asset.sha256) {
        download(&directory, &asset, &artifact).await?;
    }
    if asset.archive {
        // Re-extract only the fixed member from the checksum-verified archive.
        // Keeping that archive avoids trusting a mutable extracted-hash sidecar.
        let mut child = tokio::process::Command::new("/usr/bin/tar")
            .args(["-xOzf"])
            .arg(&artifact)
            .arg("cloudflared")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(|_| Error::Unavailable)?;
        let stdout = child.stdout.take().ok_or(Error::Unavailable)?;
        let temporary = tempfile::NamedTempFile::new_in(&directory).map_err(|_| Error::Storage)?;
        let mut destination =
            tokio::fs::File::from_std(temporary.reopen().map_err(|_| Error::Storage)?);
        let copied = tokio::time::timeout(
            Duration::from_secs(30),
            tokio::io::copy(&mut stdout.take(MAX_BYTES + 1), &mut destination),
        )
        .await
        .map_err(|_| Error::Unavailable)?
        .map_err(|_| Error::Storage)?;
        if copied == 0 || copied > MAX_BYTES {
            return Err(Error::Invalid);
        }
        let status = tokio::time::timeout(Duration::from_secs(5), child.wait())
            .await
            .map_err(|_| Error::Unavailable)?
            .map_err(|_| Error::Unavailable)?;
        if !status.success() {
            return Err(Error::Invalid);
        }
        destination.sync_all().await.map_err(|_| Error::Storage)?;
        drop(destination);
        executable(temporary.path())?;
        temporary.persist(&target).map_err(|_| Error::Storage)?;
    }
    executable(&target)?;
    #[cfg(unix)]
    std::fs::File::open(&directory)
        .and_then(|f| f.sync_all())
        .map_err(|_| Error::Storage)?;
    Ok(target)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn all_supported_pins_and_cached_artifact_integrity() {
        for os in ["linux", "macos"] {
            for arch in ["x86_64", "aarch64"] {
                let asset = asset(os, arch).unwrap();
                assert_eq!(asset.sha256.len(), 64);
            }
        }
        assert!(asset("windows", "x86_64").is_err());
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("binary");
        std::fs::write(&path, b"verified-content").unwrap();
        let expected = format!("{:x}", Sha256::digest(b"verified-content"));
        assert!(verified(&path, &expected));
        std::fs::write(&path, b"modified-content").unwrap();
        assert!(!verified(&path, &expected));
    }
    #[tokio::test]
    #[ignore = "Downloads the pinned public connector; run explicitly at integration milestones"]
    async fn pinned_installer_runs_the_verified_connector() {
        let root = tempfile::tempdir().unwrap();
        let binary = ensure(root.path()).await.unwrap();
        let result = tokio::process::Command::new(&binary)
            .arg("--version")
            .output()
            .await
            .unwrap();
        assert!(result.status.success());
        assert!(String::from_utf8_lossy(&result.stdout).contains(VERSION));
        let cached = ensure(root.path()).await.unwrap();
        assert_eq!(cached, binary);
    }
}
