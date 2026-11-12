//! Moves this computer to the server-pinned runtime release without a new image.
//!
//! Contract: `zuse_protocol::cloud_runtime_updates`; design:
//! `internal-docs/cloud-runtime-updates.md`.
//!
//! ```text
//! poll ── UpdateOffer ──▶ verify signature + architecture (before download)
//!      ──▶ download (presigned, no bearer) ──▶ zstd ──▶ sha256 ──▶ stage
//! boundary (cold boot, sleep, or `now` once quiescent):
//!   root base   : write request.json ──▶ root helper verifies, renames, writes result.json
//!   owned binary: copy ──▶ verify ──▶ probe ──▶ journal ──▶ rename (keeps `previous`)
//! ──▶ exec the installed path ──▶ new runtime starts ──▶ attest once per update id
//! ```
//!
//! Files (all JSON written by write-temp-then-rename):
//! - `<data>/runtime-updates/staged/<sha256>/zuse-cloud-runtime` + `release.json`
//!   `{"release":SignedRelease,"signature","updateId"}`
//! - `<data>/runtime-updates/request.json` `{"sha256","updateId","requestedAt"}`
//!   (root bases only; consumed by the root helper)
//! - `<data>/runtime-updates/pending.json`: the swap this runtime asked for
//! - `<data>/runtime-updates/state.json`: attested and rejected update ids
//! - root result (read only): `/var/lib/zuse/runtime-update/result.json`
//! - owned binary journal: `<binary>/../../runtime-update/journal.json` and
//!   `previous` (on Prized `~/.zuse/runtime-update/`).
use anyhow::{Result, ensure};
use ed25519_dalek::VerifyingKey;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::path::{Path, PathBuf};

pub mod agent;
pub mod stage;
pub mod swap;
pub mod verify;

pub use agent::{Activity, Ready, Updater};

pub const ARTIFACT_INVALID: &str = "runtime_update_artifact_invalid";
pub const DOWNLOAD_FAILED: &str = "runtime_update_download_failed";
pub const STORAGE_INCOMPATIBLE: &str = "runtime_update_storage_incompatible";
pub const PROBE_FAILED: &str = "runtime_update_probe_failed";
pub const ROLLED_BACK: &str = "runtime_update_rolled_back";
pub const SWAP_FAILED: &str = "runtime_update_swap_failed";
pub const SWAP_UNCONFIRMED: &str = "runtime_update_swap_unconfirmed";
pub const RESULT_MISMATCH: &str = "runtime_update_result_mismatch";
/// This computer has neither an owned binary nor the root helper.
pub const UNAVAILABLE: &str = "runtime_update_unavailable";

/// Root helper installed by bases that support in-place updates.
pub const ROOT_HELPER: &str = "/usr/local/libexec/zuse/runtime-update.py";
/// World-readable outcome the root helper writes after each request.
pub const ROOT_RESULT: &str = "/var/lib/zuse/runtime-update/result.json";

/// How this computer replaces its runtime binary.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Layout {
    /// The binary is root-owned; a root helper performs the rename.
    Root { result: PathBuf },
    /// The binary and its directory belong to this user (Prized); the runtime
    /// swaps it itself and keeps its own journal.
    Owned { journal: PathBuf },
    /// Neither: this computer cannot update in place.
    Unsupported,
}

#[derive(Clone)]
pub struct Config {
    /// `ZUSE_USER_DATA`; the staging root is `<data>/runtime-updates`.
    pub data: PathBuf,
    /// The installed runtime path (captured before any swap).
    pub binary: PathBuf,
    pub layout: Layout,
    pub verifying_key: VerifyingKey,
    pub architecture: String,
}
impl Config {
    /// The running binary, its layout, and the production release key.
    pub fn detect(data: &Path) -> Result<Self> {
        let binary = std::env::current_exe()?;
        Ok(Self {
            data: data.into(),
            layout: detect_layout(&binary, Path::new(ROOT_HELPER), Path::new(ROOT_RESULT)),
            binary,
            verifying_key: verify::release_key(
                zuse_protocol::cloud_runtime_updates::RELEASE_SIGNING_PUBLIC_KEY,
            )?,
            architecture: std::env::consts::ARCH.into(),
        })
    }
    pub fn root(&self) -> PathBuf {
        self.data.join("runtime-updates")
    }
}

/// Owned (writable binary and directory) wins; otherwise a root helper must exist.
pub fn detect_layout(binary: &Path, helper: &Path, result: &Path) -> Layout {
    use std::os::unix::fs::MetadataExt;
    let owned = binary
        .symlink_metadata()
        .is_ok_and(|m| m.is_file() && m.uid() == unsafe { libc::geteuid() })
        && binary.parent().is_some_and(writable_directory);
    if owned {
        if let Some(root) = binary.parent().and_then(Path::parent) {
            return Layout::Owned {
                journal: root.join("runtime-update"),
            };
        }
    }
    if helper.is_file() {
        return Layout::Root {
            result: result.into(),
        };
    }
    Layout::Unsupported
}
fn writable_directory(path: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let Ok(path) = std::ffi::CString::new(path.as_os_str().as_bytes()) else {
        return false;
    };
    unsafe { libc::access(path.as_ptr(), libc::W_OK | libc::X_OK) == 0 }
}

pub(crate) fn sha256_file(path: &Path) -> Result<String> {
    crate::capabilities::binary_sha256(path)
}
pub(crate) fn valid_sha(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}
pub(crate) fn read_json<T: DeserializeOwned>(path: &Path) -> Option<T> {
    let bytes = std::fs::read(path).ok()?;
    if bytes.len() > 64 * 1024 {
        return None;
    }
    serde_json::from_slice(&bytes).ok()
}
pub(crate) fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    if let Some(parent) = path.parent() {
        private_directory(parent)?;
    }
    crate::identity::atomic_write(path, &serde_json::to_vec(value)?)
}
pub(crate) fn private_directory(path: &Path) -> Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)?;
    ensure!(path.is_dir(), "runtime_update_directory_invalid");
    Ok(())
}

/// Ids this runtime already attested (never repeated) or refused (never retried).
#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct State {
    #[serde(default)]
    pub attested: Vec<String>,
    #[serde(default)]
    pub rejected: Vec<String>,
    #[serde(default)]
    pub rejected_urls: Vec<String>,
}
impl State {
    pub fn load(config: &Config) -> Self {
        read_json(&config.root().join("state.json")).unwrap_or_default()
    }
    pub fn save(&self, config: &Config) -> Result<()> {
        write_json(&config.root().join("state.json"), self)
    }
    pub fn push(list: &mut Vec<String>, value: &str) {
        if !list.iter().any(|v| v == value) {
            list.push(value.into());
        }
        let excess = list.len().saturating_sub(32);
        list.drain(..excess);
    }
}

/// Set by a swap that installed a new binary; `main` execs it after a clean shutdown.
static EXEC: std::sync::Mutex<Option<PathBuf>> = std::sync::Mutex::new(None);
pub fn request_exec(binary: PathBuf) {
    *EXEC
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(binary);
}
pub fn take_exec() -> Option<PathBuf> {
    EXEC.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .take()
}

/// Replace this process with `binary`, keeping arguments and environment, so
/// the resident supervisor still owns the same child. Returns only on failure.
pub fn exec(binary: &Path) -> std::io::Error {
    use std::os::unix::process::CommandExt;
    stop_children();
    let mut args = std::env::args_os();
    let mut command = std::process::Command::new(binary);
    if let Some(first) = args.next() {
        command.arg0(first);
    }
    command.args(args);
    // Identity is already stored; a consumed boot token file must not block restart.
    if std::env::var_os("ZUSE_RUNTIME_BOOT_TOKEN_FILE").is_some_and(|p| !Path::new(&p).is_file()) {
        command.env_remove("ZUSE_RUNTIME_BOOT_TOKEN_FILE");
    }
    tracing::info!(binary = %binary.display(), "Restarting into the updated runtime");
    command.exec()
}
/// Engine shutdown already awaited agents, terminals and services; end any
/// direct child that is somehow still running so none outlives the old image.
fn stop_children() {
    #[cfg(target_os = "linux")]
    {
        let mut children = Vec::new();
        if let Ok(tasks) = std::fs::read_dir("/proc/self/task") {
            for task in tasks.flatten() {
                if let Ok(text) = std::fs::read_to_string(task.path().join("children")) {
                    children.extend(
                        text.split_whitespace()
                            .filter_map(|p| p.parse::<i32>().ok()),
                    );
                }
            }
        }
        for pid in children {
            unsafe {
                libc::kill(pid, libc::SIGKILL);
                libc::waitpid(pid, std::ptr::null_mut(), 0);
            }
        }
    }
}

/// Cold-boot boundary, before enrollment: finish or roll back an owned swap,
/// then install an already verified staged release (never downloads). Returns
/// the binary to exec when this process must restart.
pub fn cold_boot(data: &Path) -> Option<PathBuf> {
    let _ = DATA.set(data.into());
    let config = Config::detect(data).ok()?;
    let journal = matches!(&config.layout, Layout::Owned { journal }
        if journal.join("journal.json").is_file());
    // Hash this binary only when there is something to decide.
    if !journal && stage::latest(&config).is_none() {
        return None;
    }
    let result = swap::running_sha()
        .and_then(|own| swap::cold_boot(&config, &own, std::time::Duration::from_secs(20)));
    match result {
        Ok(next) => next,
        Err(error) => {
            tracing::warn!(%error, "runtime_update_cold_boot_skipped");
            None
        }
    }
}
/// Data directory of the serving process, recorded at cold boot.
static DATA: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

/// The runtime passed its startup checks: an owned swap in probation commits.
pub fn startup_healthy() {
    let Some(config) = DATA.get().and_then(|data| Config::detect(data).ok()) else {
        return;
    };
    if let Err(error) = swap::commit_probation(&config) {
        tracing::warn!(%error, "runtime_update_commit_failed");
    }
}

/// The runtime failed before its startup checks passed: restore `previous`
/// if this start was an owned swap's probation. Returns the binary to exec.
pub fn failed_start() -> Option<PathBuf> {
    let config = Config::detect(DATA.get()?).ok()?;
    swap::rollback_probation(&config).ok().flatten()
}

#[cfg(test)]
mod tests;
