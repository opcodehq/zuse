//! Staging layout under `<data>/runtime-updates`. A staged directory appears
//! only by rename, complete and verified; a reader never sees a partial file.
use super::{ARTIFACT_INVALID, Config, read_json, verify, write_json};
use anyhow::{Result, anyhow, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::{File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};
use zuse_protocol::cloud_runtime_updates::SignedRelease;

pub const BINARY: &str = "zuse-cloud-runtime";
/// Largest uncompressed runtime accepted (same cap as the capability receipt).
pub const MAX_BINARY_BYTES: u64 = 512 * 1024 * 1024;

/// `release.json`: `RuntimeArtifact.release` + `signature` + `updateId`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseFile {
    pub release: SignedRelease,
    pub signature: String,
    pub update_id: String,
}
pub struct Staged {
    pub dir: PathBuf,
    pub binary: PathBuf,
    pub file: ReleaseFile,
}

pub fn staged_root(config: &Config) -> PathBuf {
    config.root().join("staged")
}

/// A staged release whose `release.json` still verifies. The binary digest is
/// checked again by whoever installs it (the root helper, or the owned swap on
/// its private copy), so this stays cheap enough for cold boot.
pub fn load(config: &Config, sha256: &str) -> Option<Staged> {
    if !super::valid_sha(sha256) {
        return None;
    }
    let dir = staged_root(config).join(sha256);
    let file: ReleaseFile = read_json(&dir.join("release.json"))?;
    verify::release(
        &file.release,
        &file.signature,
        &config.verifying_key,
        &config.architecture,
    )
    .ok()?;
    let binary = dir.join(BINARY);
    (file.release.sha256 == sha256 && binary.is_file()).then_some(Staged { dir, binary, file })
}
pub fn latest(config: &Config) -> Option<Staged> {
    let mut found: Vec<(std::time::SystemTime, Staged)> = std::fs::read_dir(staged_root(config))
        .ok()?
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            let staged = load(config, &name)?;
            let modified = std::fs::metadata(staged.dir.join("release.json"))
                .and_then(|m| m.modified())
                .ok()?;
            Some((modified, staged))
        })
        .collect();
    found.sort_by_key(|(modified, _)| *modified);
    found.pop().map(|(_, staged)| staged)
}
pub fn discard(config: &Config, sha256: &str) {
    if super::valid_sha(sha256) {
        let _ = std::fs::remove_dir_all(staged_root(config).join(sha256));
    }
}
/// Keep at most one staged release.
pub fn discard_others(config: &Config, keep: Option<&str>) {
    if let Ok(entries) = std::fs::read_dir(staged_root(config)) {
        for entry in entries.flatten() {
            if keep != entry.file_name().to_str() {
                let _ = std::fs::remove_dir_all(entry.path());
            }
        }
    }
}

/// A fresh private directory on the same filesystem as `staged/`.
pub fn incoming(config: &Config) -> Result<PathBuf> {
    let root = config.root().join("incoming");
    super::private_directory(&root)?;
    let dir = root.join(uuid::Uuid::new_v4().to_string());
    super::private_directory(&dir)?;
    Ok(dir)
}
pub fn clear_incoming(config: &Config) {
    let _ = std::fs::remove_dir_all(config.root().join("incoming"));
}

/// Decompress `compressed` into `<dir>/zuse-cloud-runtime`, hashing as it goes.
pub fn decompress(compressed: &Path, dir: &Path, release: &SignedRelease) -> Result<PathBuf> {
    let output = dir.join(BINARY);
    let mut decoder = zstd::stream::read::Decoder::new(File::open(compressed)?)
        .map_err(|_| anyhow!(ARTIFACT_INVALID))?;
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode_0700()
        .open(&output)?;
    let mut hash = Sha256::new();
    let mut total = 0u64;
    let mut buffer = vec![0u8; 256 * 1024];
    loop {
        let count = decoder
            .read(&mut buffer)
            .map_err(|_| anyhow!(ARTIFACT_INVALID))?;
        if count == 0 {
            break;
        }
        total += count as u64;
        ensure!(total <= MAX_BINARY_BYTES, ARTIFACT_INVALID);
        hash.update(&buffer[..count]);
        file.write_all(&buffer[..count])?;
    }
    ensure!(
        format!("{:x}", hash.finalize()) == release.sha256,
        ARTIFACT_INVALID
    );
    file.sync_all()?;
    Ok(output)
}

/// Move a verified binary (already inside an `incoming` directory) into
/// `staged/<sha256>/` with its `release.json`.
pub fn stage(config: &Config, file: ReleaseFile, binary: &Path) -> Result<Staged> {
    let dir = binary
        .parent()
        .ok_or_else(|| anyhow!("runtime_update_stage_invalid"))?
        .to_path_buf();
    ensure!(
        binary.file_name() == Some(std::ffi::OsStr::new(BINARY)),
        "runtime_update_stage_invalid"
    );
    verify::binary(binary, &file.release)?;
    for entry in std::fs::read_dir(&dir)?.flatten() {
        if entry.file_name() != BINARY {
            std::fs::remove_file(entry.path())?;
        }
    }
    std::fs::set_permissions(binary, std::os::unix::fs::PermissionsExt::from_mode(0o755))?;
    write_json(&dir.join("release.json"), &file)?;
    let root = staged_root(config);
    super::private_directory(&root)?;
    let target = root.join(&file.release.sha256);
    let _ = std::fs::remove_dir_all(&target);
    std::fs::rename(&dir, &target)?;
    File::open(&root)?.sync_all()?;
    discard_others(config, Some(&file.release.sha256));
    Ok(Staged {
        binary: target.join(BINARY),
        dir: target,
        file,
    })
}

/// `request.json`, consumed by the root helper's path unit.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Request {
    pub sha256: String,
    pub update_id: String,
    pub requested_at: i64,
}
pub fn request_path(config: &Config) -> PathBuf {
    config.root().join("request.json")
}
pub fn write_request(config: &Config, request: &Request) -> Result<()> {
    write_json(&request_path(config), request)
}
pub fn remove_request(config: &Config) {
    let _ = std::fs::remove_file(request_path(config));
}

/// The swap this runtime started; the serving runtime attests its outcome.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Pending {
    pub update_id: String,
    pub sha256: String,
    pub version: String,
    pub previous_sha256: String,
    pub requested_at: i64,
}
pub fn read_pending(config: &Config) -> Option<Pending> {
    read_json(&config.root().join("pending.json"))
}
pub fn write_pending(config: &Config, pending: &Pending) -> Result<()> {
    write_json(&config.root().join("pending.json"), pending)
}
pub fn clear_pending(config: &Config) {
    let _ = std::fs::remove_file(config.root().join("pending.json"));
}

trait PrivateMode {
    fn mode_0700(&mut self) -> &mut Self;
}
impl PrivateMode for OpenOptions {
    fn mode_0700(&mut self) -> &mut Self {
        std::os::unix::fs::OpenOptionsExt::mode(self, 0o700)
    }
}
