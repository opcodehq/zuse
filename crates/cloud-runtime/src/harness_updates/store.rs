//! Side-by-side bundle store and per-agent bindings. Every state change is
//! written to `state.json` (write-temp-then-rename) before it takes effect.
use super::{
    ADAPTER_INCOMPATIBLE, ARTIFACT_INVALID, BUNDLE_MISSING, BundleFile, Config, INSTALL_FAILED,
    PROBE_FAILED, STATE_INCOMPATIBLE, read_json, verify_bundle, write_json,
};
use anyhow::{Result, anyhow, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
    time::{Duration, Instant},
};
use zuse_protocol::cloud_harness_updates::{
    self as wire, HarnessAttestation, HarnessBinding, HarnessOutcome, SignedHarnessBundle,
};

/// Largest uncompressed bundle tar and member count accepted.
pub const MAX_TAR_BYTES: u64 = 1024 * 1024 * 1024;
pub const MAX_MEMBERS: usize = 1024;
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(20);

/// The bundle new processes of one agent start from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Active {
    pub sha256: String,
    pub version: String,
    pub state_format: String,
    pub executable: String,
    /// Kept on disk (with its files) as the fallback and rollback target.
    pub previous_sha256: Option<String>,
    pub adopted_at: i64,
}

/// A verified, installed bundle waiting for its boundary.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Staged {
    pub sha256: String,
    pub update_id: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct State {
    pub active: BTreeMap<String, Active>,
    pub staged: BTreeMap<String, Staged>,
    /// Attestations not yet delivered, oldest first.
    pub attestations: Vec<HarnessAttestation>,
    /// Update ids already handled (never retried) and URLs that served an
    /// invalid artifact (never fetched again).
    pub handled: Vec<String>,
    pub rejected_urls: Vec<String>,
}
fn push_bounded(list: &mut Vec<String>, value: &str) {
    if !list.iter().any(|v| v == value) {
        list.push(value.into());
    }
    let excess = list.len().saturating_sub(64);
    list.drain(..excess);
}

pub struct Harnesses {
    config: Config,
    state: RwLock<State>,
}

impl Harnesses {
    /// Load the store and adopt anything staged: at process start no harness
    /// process exists yet, so this is always a safe boundary.
    pub fn open(config: Config) -> Arc<Self> {
        let state = read_json(&config.root.join("state.json")).unwrap_or_default();
        let this = Arc::new(Self {
            config,
            state: RwLock::new(state),
        });
        if let Err(error) = this.adopt_staged() {
            tracing::warn!(%error, "harness_update_cold_adoption_failed");
        }
        this.collect_garbage();
        this
    }
    pub fn config(&self) -> &Config {
        &self.config
    }
    pub fn state(&self) -> State {
        self.state
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
    fn update<T>(&self, change: impl FnOnce(&mut State) -> T) -> Result<T> {
        let mut state = self
            .state
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut next = state.clone();
        let result = change(&mut next);
        write_json(&self.config.root.join("state.json"), &next)?;
        *state = next;
        Ok(result)
    }

    pub fn bundle_dir(&self, sha256: &str) -> PathBuf {
        self.config.bundles().join(sha256)
    }
    /// The bundle's files and verified metadata are on disk.
    pub fn installed(&self, sha256: &str) -> bool {
        self.bundle_dir(sha256).is_dir() && self.bundle_file(sha256).is_some()
    }
    fn bundle_file(&self, sha256: &str) -> Option<BundleFile> {
        if !wire::sha256_valid(sha256) {
            return None;
        }
        let file: BundleFile = read_json(&self.config.bundles().join(format!("{sha256}.json")))?;
        verify_bundle(
            &file.bundle,
            &file.signature,
            &self.config.verifying_key,
            &self.config.architecture,
        )
        .ok()?;
        (file.bundle.sha256 == sha256).then_some(file)
    }

    /// The CLI path a **new** process of `agent` starts from: the active
    /// bundle, else the previous bundle, else `None` (the image's CLI).
    pub fn executable(&self, agent: &str) -> Option<PathBuf> {
        let state = self
            .state
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let active = state.active.get(agent)?;
        let path = self.bundle_dir(&active.sha256).join(&active.executable);
        if path.is_file() {
            return Some(path);
        }
        let previous = active.previous_sha256.as_deref()?;
        let file = self.bundle_file(previous)?;
        let path = self.bundle_dir(previous).join(&file.bundle.executable);
        path.is_file().then_some(path)
    }

    /// What new processes of `agent` run now, if known.
    pub fn current(&self, agent: &str) -> Option<HarnessBinding> {
        if let Some(active) = self.state().active.get(agent) {
            return Some(HarnessBinding {
                sha256: Some(active.sha256.clone()),
                version: active.version.clone(),
                state_format: active.state_format.clone(),
            });
        }
        self.config.agent(agent)?.image.clone()
    }

    /// The poll body: every configured agent whose current CLI is known.
    pub fn poll(&self) -> wire::HarnessPoll {
        wire::HarnessPoll {
            architecture: self.config.architecture.clone(),
            agents: self
                .config
                .agents
                .iter()
                .filter_map(|a| {
                    let current = self.current(&a.agent)?;
                    Some(wire::AgentHarness {
                        adapters: vec![wire::adapter_for(&a.agent)?.into()],
                        agent: a.agent.clone(),
                        current: Some(current),
                    })
                })
                .collect(),
        }
    }

    /// Can this computer adopt `bundle`? Checked before download and again
    /// at adoption. Errors carry the attestation code.
    pub fn compatible(&self, bundle: &SignedHarnessBundle) -> Result<()> {
        ensure!(
            self.config.agent(&bundle.agent).is_some()
                && wire::adapter_for(&bundle.agent) == Some(bundle.adapter.as_str()),
            ADAPTER_INCOMPATIBLE
        );
        let current = self
            .current(&bundle.agent)
            .ok_or_else(|| anyhow!(STATE_INCOMPATIBLE))?;
        ensure!(
            bundle.reads_state_formats.contains(&current.state_format),
            STATE_INCOMPATIBLE
        );
        Ok(())
    }

    pub fn handled(&self, update_id: &str, url: &str) -> bool {
        let state = self.state();
        state.handled.iter().any(|id| id == update_id)
            || state.rejected_urls.iter().any(|u| u == url)
            || state.staged.values().any(|s| s.update_id == update_id)
    }
    pub fn has_staged(&self) -> bool {
        !self.state().staged.is_empty()
    }

    /// Install a verified bundle tree (in a private directory on the same
    /// filesystem) as `bundles/<sha256>` and stage it for its agent.
    pub fn stage(&self, tree: &Path, file: BundleFile) -> Result<()> {
        let sha = file.bundle.sha256.clone();
        let target = self.bundle_dir(&sha);
        if self.installed(&sha) {
            // Already installed (for example the previous bundle): reuse it.
            let _ = std::fs::remove_dir_all(tree);
        } else {
            seal(tree)?;
            super::private_directory(&self.config.bundles())?;
            remove_tree(&target);
            std::fs::rename(tree, &target).map_err(|_| anyhow!(INSTALL_FAILED))?;
            write_json(&self.config.bundles().join(format!("{sha}.json")), &file)?;
        }
        let agent = file.bundle.agent.clone();
        self.update(|state| {
            state.staged.insert(
                agent,
                Staged {
                    sha256: sha,
                    update_id: file.update_id.clone(),
                },
            );
        })
    }

    /// Record an attempt that changed nothing, for attestation.
    pub fn fail(
        &self,
        update_id: &str,
        bundle: &SignedHarnessBundle,
        code: &str,
        url: Option<&str>,
    ) {
        let attestation = HarnessAttestation {
            update_id: Some(update_id.into()),
            outcome: HarnessOutcome::Failed,
            agent: bundle.agent.clone(),
            sha256: bundle.sha256.clone(),
            version: bundle.version.clone(),
            previous_sha256: None,
            error_code: Some(code.into()),
        };
        let _ = self.update(|state| {
            push_bounded(&mut state.handled, update_id);
            if let Some(url) = url {
                push_bounded(&mut state.rejected_urls, url);
            }
            state.attestations.push(attestation);
            let excess = state.attestations.len().saturating_sub(16);
            state.attestations.drain(..excess);
        });
    }

    /// Adopt every staged bundle that is still installed and compatible. The
    /// caller guarantees a boundary: no turn is running and no command lacks
    /// an acknowledged outcome. Returns the agents that moved.
    pub fn adopt_staged(&self) -> Result<Vec<String>> {
        let staged = self.state().staged;
        let mut adopted = Vec::new();
        for (agent, staged) in staged {
            let file = self.bundle_file(&staged.sha256);
            let checked = match &file {
                Some(file) if file.bundle.agent == agent => {
                    self.compatible(&file.bundle).and_then(|()| {
                        let path = self
                            .bundle_dir(&staged.sha256)
                            .join(&file.bundle.executable);
                        ensure!(path.is_file(), BUNDLE_MISSING);
                        Ok(())
                    })
                }
                _ => Err(anyhow!(BUNDLE_MISSING)),
            };
            match (checked, file) {
                (Ok(()), Some(file)) => {
                    let bundle = file.bundle;
                    self.update(|state| {
                        let previous = state
                            .active
                            .get(&agent)
                            .map(|a| a.sha256.clone())
                            .filter(|sha| *sha != bundle.sha256);
                        state.active.insert(
                            agent.clone(),
                            Active {
                                sha256: bundle.sha256.clone(),
                                version: bundle.version.clone(),
                                state_format: bundle.state_format.clone(),
                                executable: bundle.executable.clone(),
                                previous_sha256: previous.clone(),
                                adopted_at: crate::now_ms(),
                            },
                        );
                        state.staged.remove(&agent);
                        push_bounded(&mut state.handled, &staged.update_id);
                        state.attestations.push(HarnessAttestation {
                            update_id: Some(staged.update_id.clone()),
                            outcome: HarnessOutcome::Adopted,
                            agent: agent.clone(),
                            sha256: bundle.sha256.clone(),
                            version: bundle.version.clone(),
                            previous_sha256: previous,
                            error_code: None,
                        });
                    })?;
                    tracing::info!(agent, version = %bundle.version, "Harness bundle adopted for new sessions");
                    adopted.push(agent);
                }
                (Err(error), file) => {
                    let code = known_code(&error.to_string());
                    tracing::warn!(agent, code, "harness_update_adoption_refused");
                    self.update(|state| {
                        state.staged.remove(&agent);
                    })?;
                    if let Some(file) = file {
                        self.fail(&staged.update_id, &file.bundle, code, None);
                    }
                }
                (Ok(()), None) => unreachable!("checked bundles have a file"),
            }
        }
        if !adopted.is_empty() {
            self.collect_garbage();
        }
        Ok(adopted)
    }

    pub fn pending_attestations(&self) -> Vec<HarnessAttestation> {
        self.state().attestations
    }
    pub fn attested(&self, attestation: &HarnessAttestation) {
        let _ = self.update(|state| state.attestations.retain(|a| a != attestation));
    }

    /// Remove bundles that are neither active, previous nor staged and that
    /// no live process was started from. Keeps everything a running or
    /// memory-suspended session uses.
    pub fn collect_garbage(&self) {
        let state = self.state();
        let mut keep: BTreeSet<String> = BTreeSet::new();
        for active in state.active.values() {
            keep.insert(active.sha256.clone());
            keep.extend(active.previous_sha256.clone());
        }
        keep.extend(state.staged.values().map(|s| s.sha256.clone()));
        let in_use = executables_in_use();
        let Ok(entries) = std::fs::read_dir(self.config.bundles()) else {
            return;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let sha = name.strip_suffix(".json").unwrap_or(&name).to_owned();
            if keep.contains(&sha) {
                continue;
            }
            let dir = self.bundle_dir(&sha);
            if in_use.iter().any(|exe| exe.starts_with(&dir)) {
                continue;
            }
            if name.ends_with(".json") {
                if !dir.exists() {
                    let _ = std::fs::remove_file(entry.path());
                }
            } else {
                remove_tree(&entry.path());
                let _ = std::fs::remove_file(self.config.bundles().join(format!("{sha}.json")));
            }
        }
    }
}

fn known_code(message: &str) -> &'static str {
    match message {
        ADAPTER_INCOMPATIBLE => ADAPTER_INCOMPATIBLE,
        STATE_INCOMPATIBLE => STATE_INCOMPATIBLE,
        BUNDLE_MISSING => BUNDLE_MISSING,
        PROBE_FAILED => PROBE_FAILED,
        INSTALL_FAILED => INSTALL_FAILED,
        _ => ARTIFACT_INVALID,
    }
}
pub(crate) fn code_of(error: &anyhow::Error) -> &'static str {
    known_code(&error.to_string())
}

/// Executables of live processes this user can see (`/proc/<pid>/exe`).
fn executables_in_use() -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter(|e| {
            e.file_name()
                .to_string_lossy()
                .bytes()
                .all(|b| b.is_ascii_digit())
        })
        .filter_map(|e| std::fs::read_link(e.path().join("exe")).ok())
        .collect()
}

/// Make a tree writable again and remove it.
fn remove_tree(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    if !path.exists() {
        return;
    }
    let mut stack = vec![path.to_path_buf()];
    while let Some(dir) = stack.pop() {
        if let Ok(meta) = std::fs::symlink_metadata(&dir) {
            if meta.is_dir() {
                let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
                if let Ok(entries) = std::fs::read_dir(&dir) {
                    stack.extend(entries.flatten().map(|e| e.path()));
                }
            }
        }
    }
    let _ = std::fs::remove_dir_all(path);
}

/// Directories become read-only once their files are in place.
fn seal(tree: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut dirs = vec![tree.to_path_buf()];
    let mut index = 0;
    while index < dirs.len() {
        for entry in std::fs::read_dir(&dirs[index])?.flatten() {
            if entry.file_type()?.is_dir() {
                dirs.push(entry.path());
            }
        }
        index += 1;
    }
    for dir in dirs.iter().rev() {
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o555))?;
    }
    Ok(())
}

/// Decompress `compressed` to `tar`, hashing as it goes; the digest must be
/// the signed `sha256`.
pub fn decompress(compressed: &Path, tar: &Path, sha256: &str) -> Result<()> {
    let mut decoder = zstd::stream::read::Decoder::new(std::fs::File::open(compressed)?)
        .map_err(|_| anyhow!(ARTIFACT_INVALID))?;
    let mut output = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(tar)?;
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
        ensure!(total <= MAX_TAR_BYTES, ARTIFACT_INVALID);
        hash.update(&buffer[..count]);
        output.write_all(&buffer[..count])?;
    }
    ensure!(format!("{:x}", hash.finalize()) == sha256, ARTIFACT_INVALID);
    output.sync_all()?;
    Ok(())
}

/// Extract a verified bundle tar into the empty directory `tree`: regular
/// files and directories only, safe relative paths, no links or devices.
pub fn extract(tar: &Path, tree: &Path, bundle: &SignedHarnessBundle) -> Result<PathBuf> {
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    let mut archive = ::tar::Archive::new(std::fs::File::open(tar)?);
    let mut members = 0usize;
    for entry in archive.entries().map_err(|_| anyhow!(ARTIFACT_INVALID))? {
        let mut entry = entry.map_err(|_| anyhow!(ARTIFACT_INVALID))?;
        members += 1;
        ensure!(members <= MAX_MEMBERS, ARTIFACT_INVALID);
        let name = entry
            .path()
            .map_err(|_| anyhow!(ARTIFACT_INVALID))?
            .to_str()
            .map(|s| s.trim_end_matches('/').to_owned())
            .ok_or_else(|| anyhow!(ARTIFACT_INVALID))?;
        ensure!(wire::path_valid(&name), ARTIFACT_INVALID);
        let target = tree.join(&name);
        match entry.header().entry_type() {
            ::tar::EntryType::Directory => {
                std::fs::create_dir_all(&target)?;
            }
            ::tar::EntryType::Regular => {
                if let Some(parent) = target.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                let executable = entry.header().mode().unwrap_or(0) & 0o111 != 0;
                let mut output = std::fs::OpenOptions::new()
                    .create_new(true)
                    .write(true)
                    .mode(0o600)
                    .open(&target)?;
                std::io::copy(&mut entry, &mut output)?;
                output.sync_all()?;
                std::fs::set_permissions(
                    &target,
                    std::fs::Permissions::from_mode(if executable { 0o555 } else { 0o444 }),
                )?;
            }
            // PAX headers are consumed by the reader; anything else is refused.
            _ => return Err(anyhow!(ARTIFACT_INVALID)),
        }
    }
    let executable = tree.join(&bundle.executable);
    ensure!(
        executable
            .symlink_metadata()
            .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0),
        ARTIFACT_INVALID
    );
    Ok(executable)
}

/// Run `--version` with no credentials, an empty home and updates disabled;
/// the output must name the exact bundle version.
pub fn probe(executable: &Path, version: &str) -> Result<()> {
    let home = tempfile_dir(executable)?;
    let result = (|| {
        let mut child = std::process::Command::new(executable)
            .arg("--version")
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", &home)
            .env("DISABLE_AUTOUPDATER", "1")
            .env("DISABLE_UPDATES", "1")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|_| anyhow!(PROBE_FAILED))?;
        let deadline = Instant::now() + PROBE_TIMEOUT;
        loop {
            match child.try_wait()? {
                Some(status) => {
                    ensure!(status.success(), PROBE_FAILED);
                    break;
                }
                None if Instant::now() >= deadline => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(anyhow!(PROBE_FAILED));
                }
                None => std::thread::sleep(Duration::from_millis(20)),
            }
        }
        let mut output = String::new();
        if let Some(mut stdout) = child.stdout.take() {
            let _ = stdout.by_ref().take(64 * 1024).read_to_string(&mut output);
        }
        ensure!(reports_version(&output, version), PROBE_FAILED);
        Ok(())
    })();
    let _ = std::fs::remove_dir_all(&home);
    result
}
fn tempfile_dir(near: &Path) -> Result<PathBuf> {
    let dir = near
        .ancestors()
        .find(|p| p.file_name().is_some_and(|n| n == "incoming"))
        .map(Path::to_path_buf)
        .unwrap_or_else(std::env::temp_dir)
        .join(format!("probe-home-{}", uuid::Uuid::new_v4()));
    super::private_directory(&dir)?;
    Ok(dir)
}
/// `version` appears as a whole token (not as a prefix of a longer version).
pub fn reports_version(output: &str, version: &str) -> bool {
    output.match_indices(version).any(|(at, _)| {
        let before = output[..at].chars().next_back();
        let after = output[at + version.len()..].chars().next();
        !before.is_some_and(|c| c.is_alphanumeric() || c == '.' || c == '_')
            && !after.is_some_and(|c| c.is_alphanumeric() || ".+-_".contains(c))
    })
}
