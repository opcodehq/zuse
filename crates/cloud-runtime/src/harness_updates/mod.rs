//! Agent CLIs that update independently of the machine image.
//!
//! Contract: `zuse_protocol::cloud_harness_updates`; operator flow:
//! `infra/cloud/HARNESS_BUNDLES.md`. The runtime polls for signed harness
//! bundles, downloads them in the background (never on the first-turn path),
//! verifies and installs each version side by side under its digest, and
//! adopts a staged bundle only at a quiescent boundary. Adoption changes the
//! path *new* harness processes start from ([`Harnesses::executable`]); a
//! running process keeps the exact file it started with and is never killed.
//!
//! ```text
//! poll ── HarnessOffer ──▶ verify signature, agent, adapter, state format
//!      ──▶ download (presigned, no bearer) ──▶ zstd ──▶ sha256 ──▶ extract
//!      ──▶ probe `--version` ──▶ read-only `bundles/<sha256>/` ──▶ staged
//! boundary (cold start, or no turn and no unacknowledged command):
//!      staged ──▶ active (previous kept) ──▶ attest adopted
//! new harness process ──▶ Harnesses::executable(agent) ──▶ bundle path
//!                         (else the previous bundle, else the image's CLI)
//! ```
//!
//! Files under `<data>/harnesses/` (the task user owns them; agents run as
//! that user, so integrity rests on digest plus signature, as on Prized):
//! - `bundles/<sha256>/…` the extracted bundle tree (read-only)
//! - `bundles/<sha256>.json` `{"bundle","signature","updateId"}`
//! - `state.json` active and staged bundles per agent, attestations not yet
//!   delivered, and refused update ids and URLs
use anyhow::{Result, anyhow, ensure};
use ed25519_dalek::VerifyingKey;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::path::{Path, PathBuf};
use zuse_protocol::cloud_harness_updates::{self as wire, HarnessBinding, SignedHarnessBundle};

pub mod agent;
pub mod store;

pub use agent::Updater;
pub use store::Harnesses;

pub const ARTIFACT_INVALID: &str = "harness_update_artifact_invalid";
pub const DOWNLOAD_FAILED: &str = "harness_update_download_failed";
pub const PROBE_FAILED: &str = "harness_update_probe_failed";
pub const INSTALL_FAILED: &str = "harness_update_install_failed";
pub const ADAPTER_INCOMPATIBLE: &str = "harness_update_adapter_incompatible";
pub const STATE_INCOMPATIBLE: &str = "harness_update_state_incompatible";
pub const BUNDLE_MISSING: &str = "harness_update_bundle_missing";

/// How this computer verifies and binds harness bundles.
#[derive(Clone)]
pub struct Config {
    /// `<data>/harnesses`.
    pub root: PathBuf,
    pub verifying_key: VerifyingKey,
    pub architecture: String,
    /// Agents this runtime runs, with the CLI that came with the image (if
    /// known). Only these are polled for, and only once their current CLI is
    /// known, so a bundle is never adopted over an unknown session format.
    pub agents: Vec<AgentConfig>,
}
#[derive(Clone, Debug)]
pub struct AgentConfig {
    pub agent: String,
    pub image: Option<HarnessBinding>,
}
impl Config {
    /// Production configuration: the release key, this architecture, and the
    /// image CLIs from the managed harness release (`ZUSE_HARNESS_ROOT`,
    /// `ZUSE_HARNESS_RELEASE`) when present.
    pub fn detect(data: &Path, agents: &[&str]) -> Result<Self> {
        let image = image_manifest(
            std::env::var_os("ZUSE_HARNESS_ROOT").map(PathBuf::from),
            std::env::var("ZUSE_HARNESS_RELEASE").ok(),
        );
        Ok(Self {
            root: data.join("harnesses"),
            verifying_key: crate::updates::verify::release_key(
                zuse_protocol::cloud_runtime_updates::RELEASE_SIGNING_PUBLIC_KEY,
            )?,
            architecture: std::env::consts::ARCH.into(),
            agents: agents
                .iter()
                .map(|agent| AgentConfig {
                    agent: (*agent).to_owned(),
                    image: image.as_ref().and_then(|m| image_binding(m, agent)),
                })
                .collect(),
        })
    }
    pub fn agent(&self, agent: &str) -> Option<&AgentConfig> {
        self.agents.iter().find(|a| a.agent == agent)
    }
    pub fn bundles(&self) -> PathBuf {
        self.root.join("bundles")
    }
}

/// The managed harness release manifest baked into the image, if any.
pub fn image_manifest(root: Option<PathBuf>, release: Option<String>) -> Option<serde_json::Value> {
    let release = release.filter(|r| wire::sha256_valid(r))?;
    let path = root?.join("releases").join(release).join("manifest.json");
    read_json(&path)
}
/// The image CLI of `agent` described by a harness release manifest.
pub fn image_binding(manifest: &serde_json::Value, agent: &str) -> Option<HarnessBinding> {
    let version = manifest["harnesses"][agent]["version"].as_str()?;
    let state_format = manifest["stateFormat"].as_str()?;
    (wire::version_valid(version) && wire::name_valid(state_format)).then(|| HarnessBinding {
        sha256: None,
        version: version.into(),
        state_format: state_format.into(),
    })
}

/// Errors carry the attestation code as their message.
pub fn verify_bundle(
    bundle: &SignedHarnessBundle,
    signature: &str,
    key: &VerifyingKey,
    architecture: &str,
) -> Result<()> {
    use base64::{Engine as _, engine::general_purpose::STANDARD};
    let signature: [u8; 64] = STANDARD
        .decode(signature)
        .ok()
        .and_then(|b| b.try_into().ok())
        .ok_or_else(|| anyhow!(ARTIFACT_INVALID))?;
    key.verify_strict(
        &bundle.signing_bytes(),
        &ed25519_dalek::Signature::from_bytes(&signature),
    )
    .map_err(|_| anyhow!(ARTIFACT_INVALID))?;
    ensure!(
        bundle.validate().is_ok() && bundle.architecture == architecture,
        ARTIFACT_INVALID
    );
    Ok(())
}

/// Never during the first turn after a wake: memory resumes record it here.
static WOKE: std::sync::Mutex<Option<std::time::Instant>> = std::sync::Mutex::new(None);
/// A memory resume happened: restart the first-turn grace for downloads.
pub fn note_wake() {
    *WOKE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(std::time::Instant::now());
}
pub(crate) fn last_wake() -> Option<std::time::Instant> {
    *WOKE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

pub(crate) fn read_json<T: DeserializeOwned>(path: &Path) -> Option<T> {
    let bytes = std::fs::read(path).ok()?;
    if bytes.len() > 256 * 1024 {
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
    ensure!(path.is_dir(), "harness_update_directory_invalid");
    Ok(())
}

/// `bundles/<sha256>.json`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BundleFile {
    pub bundle: SignedHarnessBundle,
    pub signature: String,
    pub update_id: String,
}

#[cfg(test)]
mod tests;
