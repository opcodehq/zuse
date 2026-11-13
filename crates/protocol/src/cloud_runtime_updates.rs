//! Updating a cloud computer's guest runtime without a new machine image.
//!
//! Design: `internal-docs/cloud-runtime-updates.md`. The server pins a signed
//! runtime release per computer; the computer downloads it in the background,
//! swaps it at an idle boundary (sleep, cold boot, or immediately when a
//! message needs it) and attests the result. Old runtimes ignore all of this.
//!
//! Flow:
//! ```text
//! runtime ── GET  /v1/cloud/workspaces/{id}/runtime/update ──▶ server
//!         ◀── 200 UpdateOffer { directive: Option<UpdateDirective> }
//! runtime: download → verify sha256 + signature → stage
//! runtime: swap at boundary (urgency) → restart → probe
//! runtime ── POST /v1/cloud/workspaces/{id}/runtime/update/attestation ──▶ server
//!            RuntimeAttestation { outcome, sha256, capabilities, … }
//! ```
//! The server may also nudge a connected runtime to poll now with the gateway
//! notification [`POLL_NOW_METHOD`] (used for demand updates).
use serde::{Deserialize, Serialize};

/// The runtime can download, verify, swap, roll back and attest a release.
pub const CAPABILITY: &str = "rust-runtime-update-v1";

/// Gateway notification asking a connected runtime to poll for an update now.
pub const POLL_NOW_METHOD: &str = "runtime.update.pollNow";

/// Ed25519 public key (raw 32 bytes, standard base64) that signs runtime
/// releases. The private key never leaves the release signer.
pub const RELEASE_SIGNING_PUBLIC_KEY: &str = "CTEMp+P0mILHcWFjITtV0yipAeHMNdlho7pxbsntKd8=";

/// Admission refused a message because the computer is updating its runtime
/// to gain a feature the message needs. The message did not run; the app keeps
/// it and resends once [`UpdateStatus`] reports `committed`.
pub const RUNTIME_UPDATING: &str = "cloud_computer_runtime_updating";
/// The computer cannot update in place (no update support, repeated rollbacks,
/// or a rollback barrier). The app offers "Update computer" (recreation onto
/// the current base with `targetRelease: "latest"`).
pub const RUNTIME_UPDATE_UNAVAILABLE: &str = "cloud_computer_runtime_update_unavailable";

/// The fields a release signature covers. Signed bytes are
/// [`SignedRelease::signing_bytes`]: canonical, compact JSON with keys in the
/// order declared here.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SignedRelease {
    /// Lowercase hex SHA-256 of the uncompressed runtime binary.
    pub sha256: String,
    pub version: String,
    /// `x86_64` or `aarch64`.
    pub architecture: String,
    /// Storage/identity formats this release can read; see the design's
    /// two-phase rule.
    pub storage_formats: Vec<String>,
}
impl SignedRelease {
    pub fn signing_bytes(&self) -> Vec<u8> {
        let mut bytes = b"zuse-runtime-release-v1\n".to_vec();
        bytes.extend(serde_json::to_vec(self).expect("release serializes"));
        bytes
    }
}

/// Where and what to download. `url` is a short-lived presigned GET for a
/// zstd-compressed binary; never trust it without checking `release.sha256`
/// (after decompression) and `signature`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeArtifact {
    pub release: SignedRelease,
    /// Standard base64 Ed25519 signature over `release.signing_bytes()`.
    pub signature: String,
    pub url: String,
    pub compressed_size_bytes: u64,
    /// Unix ms after which `url` stops working; poll again for a fresh one.
    pub url_expires_at: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Urgency {
    /// Download in the background; swap at the next sleep or cold boot.
    Background,
    /// A waiting message needs this release: swap as soon as the runtime is
    /// quiescent (no running turn, no leased command without a terminal ACK).
    Now,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateDirective {
    /// Server-minted id; echoed in the attestation.
    pub update_id: String,
    pub artifact: RuntimeArtifact,
    pub urgency: Urgency,
}

/// Response to the runtime's poll. `None` means "you are on the pinned release".
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateOffer {
    pub directive: Option<UpdateDirective>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Outcome {
    /// The new runtime started, passed its probe and is serving.
    Committed,
    /// The new runtime failed; the previous one was restored and is serving.
    RolledBack,
    /// Nothing was swapped (download, digest or signature failed).
    Failed,
}

/// Sent by the runtime that is serving after an update attempt (the new one on
/// `committed`, the restored one otherwise). Replaces the computer's stored
/// capabilities, so the server never judges a swapped runtime by its old
/// bootstrap receipt.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeAttestation {
    pub update_id: Option<String>,
    pub outcome: Outcome,
    /// SHA-256 of the runtime now serving.
    pub sha256: String,
    pub version: String,
    pub capabilities: Vec<String>,
    pub previous_sha256: Option<String>,
    /// Short `[a-z0-9_-]` code when not committed, e.g.
    /// `runtime_update_artifact_invalid`, `runtime_update_rolled_back`.
    pub error_code: Option<String>,
}

/// What the app polls while a demand update runs:
/// `GET /v1/cloud/workspaces/{id}/runtime-update` (user-authenticated).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateStatus {
    pub update_id: Option<String>,
    pub state: UpdateState,
    pub error_code: Option<String>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum UpdateState {
    /// On the pinned release; nothing to do.
    Current,
    Pending,
    Downloading,
    Swapping,
    Committed,
    RolledBack,
    Failed,
    /// Cannot update in place; offer "Update computer".
    Unavailable,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn signing_bytes_are_canonical_and_domain_separated() {
        let release = SignedRelease {
            sha256: "ab".repeat(32),
            version: "1.2.3".into(),
            architecture: "x86_64".into(),
            storage_formats: vec!["zuse-rust-engine-v1".into()],
        };
        let bytes = String::from_utf8(release.signing_bytes()).unwrap();
        assert_eq!(
            bytes,
            format!(
                "zuse-runtime-release-v1\n{{\"sha256\":\"{}\",\"version\":\"1.2.3\",\"architecture\":\"x86_64\",\"storageFormats\":[\"zuse-rust-engine-v1\"]}}",
                "ab".repeat(32)
            )
        );
    }
    /// The release pipeline (`infra/cloud/runtime-release.py`) signs the same
    /// fixture; both sides must produce these exact bytes.
    #[test]
    fn signing_bytes_match_the_release_pipeline_fixture() {
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../fixtures/runtime-release-v1.json")).unwrap();
        let release: SignedRelease = serde_json::from_value(fixture["release"].clone()).unwrap();
        assert_eq!(
            String::from_utf8(release.signing_bytes()).unwrap(),
            fixture["signingBytes"].as_str().unwrap()
        );
    }
    #[test]
    fn public_key_is_32_bytes() {
        use base64::Engine as _;
        let key = base64::engine::general_purpose::STANDARD
            .decode(RELEASE_SIGNING_PUBLIC_KEY)
            .unwrap();
        assert_eq!(key.len(), 32);
    }
}
