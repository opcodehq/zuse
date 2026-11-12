//! Swap boundaries. Root bases hand the rename to the root helper through
//! `request.json`; an owned binary (Prized) is swapped here with a journal and
//! a kept `previous`, and rolls back if the new runtime never reaches health.
use super::{
    ARTIFACT_INVALID, Config, Layout, PROBE_FAILED, RESULT_MISMATCH, ROLLED_BACK, SWAP_FAILED,
    SWAP_UNCONFIRMED, State, read_json, stage, verify, write_json,
};
use anyhow::{Result, anyhow, ensure};
use serde::{Deserialize, Serialize};
use std::{
    fs::{File, OpenOptions},
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};
use zuse_protocol::cloud_runtime_updates::Outcome;

/// A root helper that acknowledged nothing for this long did not swap.
pub const UNCONFIRMED_AFTER_MS: i64 = 10 * 60 * 1000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Phase {
    /// Renamed into place; the new runtime has not started yet.
    Swapped,
    /// The new runtime started once and has not passed its startup checks.
    Probation,
    Committed,
    RolledBack,
    /// Nothing was swapped.
    Failed,
}
/// `<binary>/../../runtime-update/journal.json` (owned layout only).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Journal {
    pub version: u8,
    pub phase: Phase,
    pub update_id: String,
    pub sha256: String,
    pub previous_sha256: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    pub updated_at: i64,
}
pub fn read_journal(journal: &Path) -> Option<Journal> {
    read_json::<Journal>(&journal.join("journal.json")).filter(|j| j.version == 1)
}
fn write_journal(journal: &Path, value: &mut Journal) -> Result<()> {
    value.updated_at = crate::now_ms();
    write_json(&journal.join("journal.json"), value)
}

/// `/var/lib/zuse/runtime-update/result.json`, written by the root helper.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RootResult {
    pub update_id: String,
    pub outcome: Outcome,
    pub sha256: String,
    #[serde(default)]
    pub previous_sha256: Option<String>,
    #[serde(default)]
    pub error_code: Option<String>,
}
pub fn root_result(path: &Path, update_id: &str) -> Option<RootResult> {
    read_json::<RootResult>(path).filter(|r| r.update_id == update_id)
}

/// SHA-256 of the image this process is running, even after a rename.
pub fn running_sha() -> Result<String> {
    #[cfg(target_os = "linux")]
    let path = PathBuf::from("/proc/self/exe");
    #[cfg(not(target_os = "linux"))]
    let path = std::env::current_exe()?;
    super::sha256_file(&path)
}

/// `<binary> runtime-capabilities` exits 0 within 5 s and reports a valid
/// receipt for exactly `sha256`.
pub fn probe(binary: &Path, sha256: &str) -> bool {
    let mut attempts = 0;
    let mut child = loop {
        match Command::new(binary)
            .arg("runtime-capabilities")
            .env_clear()
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(child) => break child,
            // A just-written file can be briefly busy if another thread forked.
            Err(error) if error.raw_os_error() == Some(libc::ETXTBSY) && attempts < 20 => {
                attempts += 1;
                std::thread::sleep(Duration::from_millis(25));
            }
            Err(_) => return false,
        }
    };
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() < Duration::from_secs(5) => {
                std::thread::sleep(Duration::from_millis(10))
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return false;
            }
        }
    };
    let mut bytes = Vec::new();
    if !status.success()
        || child
            .stdout
            .take()
            .is_none_or(|out| out.take(8193).read_to_end(&mut bytes).is_err())
        || bytes.len() > 8192
    {
        return false;
    }
    serde_json::from_slice::<crate::capabilities::CommandAdmissionReceipt>(&bytes)
        .is_ok_and(|receipt| receipt.valid() && receipt.runtime_sha256.eq_ignore_ascii_case(sha256))
}

fn copy_executable(source: &Path, target: &Path) -> Result<()> {
    let _ = std::fs::remove_file(target);
    let mut input = File::open(source)?;
    let mut output = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(target)?;
    std::io::copy(&mut input, &mut output)?;
    output.set_permissions(std::os::unix::fs::PermissionsExt::from_mode(0o755))?;
    output.sync_all()?;
    Ok(())
}
fn sync_parent(path: &Path) -> Result<()> {
    File::open(
        path.parent()
            .ok_or_else(|| anyhow!("runtime_update_path_invalid"))?,
    )?
    .sync_all()?;
    Ok(())
}
fn next_path(binary: &Path) -> Result<PathBuf> {
    Ok(binary
        .parent()
        .ok_or_else(|| anyhow!("runtime_update_path_invalid"))?
        .join(".zuse-cloud-runtime.next"))
}

/// Owned layout: copy → verify the copy → probe → keep `previous` → journal
/// `swapped` → rename. `Ok(Err(code))` means nothing was swapped; the journal
/// records it as `failed` for attestation.
pub fn owned_swap(
    config: &Config,
    journal: &Path,
    staged: &stage::Staged,
    previous_sha256: &str,
) -> Result<std::result::Result<(), &'static str>> {
    let release = &staged.file.release;
    stage::write_pending(
        config,
        &stage::Pending {
            update_id: staged.file.update_id.clone(),
            sha256: release.sha256.clone(),
            version: release.version.clone(),
            previous_sha256: previous_sha256.into(),
            requested_at: crate::now_ms(),
        },
    )?;
    let mut entry = Journal {
        version: 1,
        phase: Phase::Failed,
        update_id: staged.file.update_id.clone(),
        sha256: release.sha256.clone(),
        previous_sha256: previous_sha256.into(),
        error_code: None,
        updated_at: 0,
    };
    let next = next_path(&config.binary)?;
    copy_executable(&staged.binary, &next)?;
    let refused = if verify::binary(&next, release).is_err() {
        Some(ARTIFACT_INVALID)
    } else if !probe(&next, &release.sha256) {
        Some(PROBE_FAILED)
    } else {
        None
    };
    if let Some(code) = refused {
        let _ = std::fs::remove_file(&next);
        entry.error_code = Some(code.into());
        write_journal(journal, &mut entry)?;
        return Ok(Err(code));
    }
    super::private_directory(journal)?;
    let previous = journal.join("previous");
    let previous_next = journal.join("previous.next");
    copy_executable(&config.binary, &previous_next)?;
    ensure!(
        super::sha256_file(&previous_next)? == previous_sha256,
        "runtime_update_previous_mismatch"
    );
    std::fs::rename(&previous_next, &previous)?;
    sync_parent(&previous)?;
    entry.phase = Phase::Swapped;
    write_journal(journal, &mut entry)?;
    std::fs::rename(&next, &config.binary)?;
    sync_parent(&config.binary)?;
    Ok(Ok(()))
}

/// Restore `previous` over the binary and record `rolled-back`.
fn rollback(config: &Config, journal: &Path, entry: &mut Journal) -> Result<PathBuf> {
    let previous = journal.join("previous");
    ensure!(
        super::sha256_file(&previous)? == entry.previous_sha256,
        "runtime_update_previous_mismatch"
    );
    let next = next_path(&config.binary)?;
    copy_executable(&previous, &next)?;
    std::fs::rename(&next, &config.binary)?;
    sync_parent(&config.binary)?;
    entry.phase = Phase::RolledBack;
    entry.error_code = Some(ROLLED_BACK.into());
    write_journal(journal, entry)?;
    tracing::warn!(update = %entry.update_id, "runtime_update_rolled_back");
    Ok(config.binary.clone())
}

/// Owned layout, process start: a fresh swap enters probation; a second start
/// still in probation means the new runtime never became healthy → roll back.
pub fn start_recovery(config: &Config, journal: &Path, own: &str) -> Result<Option<PathBuf>> {
    let Some(mut entry) = read_journal(journal) else {
        return Ok(None);
    };
    match entry.phase {
        Phase::Swapped if own == entry.sha256 => {
            entry.phase = Phase::Probation;
            write_journal(journal, &mut entry)?;
        }
        Phase::Swapped if own == entry.previous_sha256 => {
            // Journal written, rename never happened.
            entry.phase = Phase::Failed;
            entry.error_code = Some(SWAP_FAILED.into());
            write_journal(journal, &mut entry)?;
        }
        Phase::Probation if own == entry.sha256 => {
            return rollback(config, journal, &mut entry).map(Some);
        }
        Phase::Probation if own == entry.previous_sha256 => {
            entry.phase = Phase::RolledBack;
            entry.error_code = Some(ROLLED_BACK.into());
            write_journal(journal, &mut entry)?;
        }
        _ => {}
    }
    Ok(None)
}
pub fn commit_with(config: &Config, own: &str) -> Result<()> {
    let Layout::Owned { journal } = &config.layout else {
        return Ok(());
    };
    if let Some(mut entry) = read_journal(journal) {
        if entry.phase == Phase::Probation && entry.sha256 == own {
            entry.phase = Phase::Committed;
            write_journal(journal, &mut entry)?;
        }
    }
    Ok(())
}
pub fn commit_probation(config: &Config) -> Result<()> {
    if in_probation(config) {
        commit_with(config, &running_sha()?)?;
    }
    Ok(())
}
fn in_probation(config: &Config) -> bool {
    matches!(&config.layout, Layout::Owned { journal }
        if read_journal(journal).is_some_and(|j| j.phase == Phase::Probation))
}
pub fn rollback_with(config: &Config, own: &str) -> Result<Option<PathBuf>> {
    let Layout::Owned { journal } = &config.layout else {
        return Ok(None);
    };
    match read_journal(journal) {
        Some(mut entry) if entry.phase == Phase::Probation && entry.sha256 == own => {
            rollback(config, journal, &mut entry).map(Some)
        }
        _ => Ok(None),
    }
}
pub fn rollback_probation(config: &Config) -> Result<Option<PathBuf>> {
    if !in_probation(config) {
        return Ok(None);
    }
    rollback_with(config, &running_sha()?)
}

/// Root layout: record the pending swap, then ask the helper.
pub fn request_root(config: &Config, staged: &stage::Staged, previous_sha256: &str) -> Result<()> {
    let now = crate::now_ms();
    stage::write_pending(
        config,
        &stage::Pending {
            update_id: staged.file.update_id.clone(),
            sha256: staged.file.release.sha256.clone(),
            version: staged.file.release.version.clone(),
            previous_sha256: previous_sha256.into(),
            requested_at: now,
        },
    )?;
    stage::write_request(
        config,
        &stage::Request {
            sha256: staged.file.release.sha256.clone(),
            update_id: staged.file.update_id.clone(),
            requested_at: now,
        },
    )
}

/// What the helper (or our own journal) decided for a swap this runtime asked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Decision {
    /// The new release is installed; restart into it if not already running it.
    Installed,
    /// Nothing changed, or the previous runtime was restored.
    Refused {
        outcome: Outcome,
        code: String,
    },
    Undecided,
}
pub fn decision(config: &Config, pending: &stage::Pending) -> Decision {
    match &config.layout {
        Layout::Owned { journal } => match read_journal(journal) {
            Some(j) if j.update_id == pending.update_id => match j.phase {
                Phase::Swapped | Phase::Probation | Phase::Committed => Decision::Installed,
                Phase::RolledBack => Decision::Refused {
                    outcome: Outcome::RolledBack,
                    code: j.error_code.unwrap_or_else(|| ROLLED_BACK.into()),
                },
                Phase::Failed => Decision::Refused {
                    outcome: Outcome::Failed,
                    code: j.error_code.unwrap_or_else(|| SWAP_FAILED.into()),
                },
            },
            _ => Decision::Undecided,
        },
        Layout::Root { result } => match root_result(result, &pending.update_id) {
            Some(r) if r.outcome == Outcome::Committed && r.sha256 == pending.sha256 => {
                Decision::Installed
            }
            Some(r) if r.outcome == Outcome::Committed => Decision::Refused {
                outcome: Outcome::Failed,
                code: RESULT_MISMATCH.into(),
            },
            Some(r) => Decision::Refused {
                outcome: r.outcome,
                code: r.error_code.unwrap_or_else(|| {
                    if r.outcome == Outcome::RolledBack {
                        ROLLED_BACK.into()
                    } else {
                        SWAP_FAILED.into()
                    }
                }),
            },
            None => Decision::Undecided,
        },
        Layout::Unsupported => Decision::Undecided,
    }
}

/// What the serving runtime attests for `pending`, or `None` while undecided.
pub fn attestation_outcome(
    config: &Config,
    pending: &stage::Pending,
    own: &str,
) -> Option<(Outcome, Option<String>)> {
    match decision(config, pending) {
        Decision::Installed if own == pending.sha256 => {
            // Owned: only after this runtime passed its startup checks.
            match &config.layout {
                Layout::Owned { journal }
                    if read_journal(journal).is_none_or(|j| j.phase != Phase::Committed) =>
                {
                    None
                }
                _ => Some((Outcome::Committed, None)),
            }
        }
        // Installed on disk, but this process is still the old image.
        Decision::Installed => None,
        Decision::Refused { outcome, code } => Some((outcome, Some(code))),
        Decision::Undecided if crate::now_ms() - pending.requested_at > UNCONFIRMED_AFTER_MS => {
            Some((Outcome::Failed, Some(SWAP_UNCONFIRMED.into())))
        }
        Decision::Undecided => None,
    }
}

/// Install `staged` now. Returns the binary to exec, or `None` when nothing
/// was installed (the outcome is left for attestation). The caller has
/// already proved quiescence.
pub fn install(
    config: &Config,
    staged: &stage::Staged,
    own: &str,
    wait: Duration,
    cancelled: &dyn Fn() -> bool,
) -> Result<Option<PathBuf>> {
    match &config.layout {
        Layout::Unsupported => Ok(None),
        Layout::Owned { journal } => Ok(match owned_swap(config, journal, staged, own)? {
            Ok(()) => Some(config.binary.clone()),
            Err(code) => {
                tracing::warn!(code, "runtime_update_not_installed");
                stage::discard(config, &staged.file.release.sha256);
                None
            }
        }),
        Layout::Root { .. } => {
            request_root(config, staged, own)?;
            let pending = stage::read_pending(config)
                .ok_or_else(|| anyhow!("runtime_update_pending_missing"))?;
            let started = Instant::now();
            while started.elapsed() < wait && !cancelled() {
                match decision(config, &pending) {
                    Decision::Installed => {
                        stage::remove_request(config);
                        return Ok(Some(config.binary.clone()));
                    }
                    Decision::Refused { code, .. } => {
                        tracing::warn!(%code, "runtime_update_not_installed");
                        stage::remove_request(config);
                        stage::discard(config, &pending.sha256);
                        return Ok(None);
                    }
                    Decision::Undecided => std::thread::sleep(Duration::from_millis(100)),
                }
            }
            // Withdraw the request so a late helper cannot restart a busy runtime.
            stage::remove_request(config);
            Ok(None)
        }
    }
}

/// Cold-boot boundary (before enrollment). Never downloads.
pub fn cold_boot(config: &Config, own: &str, wait: Duration) -> Result<Option<PathBuf>> {
    if let Layout::Owned { journal } = &config.layout {
        if let Some(binary) = start_recovery(config, journal, own)? {
            return Ok(Some(binary));
        }
    }
    if config.layout == Layout::Unsupported {
        return Ok(None);
    }
    let Some(staged) = stage::latest(config) else {
        return Ok(None);
    };
    let release = &staged.file.release;
    if release.sha256 == own {
        return Ok(None);
    }
    if State::load(config)
        .rejected
        .contains(&staged.file.update_id)
    {
        stage::discard(config, &release.sha256);
        return Ok(None);
    }
    // One attempt per staged release: an earlier request awaits attestation.
    if stage::read_pending(config).is_some_and(|p| p.sha256 == release.sha256) {
        return Ok(None);
    }
    install(config, &staged, own, wait, &|| false)
}
