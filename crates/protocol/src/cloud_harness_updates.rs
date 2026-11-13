//! Updating the agent CLIs a cloud computer runs without a new machine image.
//!
//! Operator flow: `infra/cloud/HARNESS_BUNDLES.md`. A harness bundle is one
//! exact upstream CLI version for one agent and architecture, packaged as a
//! deterministic tar, compressed with zstd and signed with the runtime release
//! key under its own domain. The server registers bundles in channels and
//! offers them; the computer downloads in the background, verifies, installs
//! side by side and adopts a bundle only at a quiescent boundary.
//!
//! ```text
//! runtime ── POST /v1/cloud/workspaces/{id}/runtime/harness-updates ──▶ server
//!            HarnessPoll { architecture, agents: [{agent, adapters, current}] }
//!         ◀── 200 HarnessOffer { directives: [HarnessDirective] }
//! runtime: download → zstd → sha256 + signature → extract → probe → stage
//! runtime: adopt at a quiescent boundary (new harness processes only)
//! runtime ── POST …/runtime/harness-updates/attestation ──▶ server
//!            HarnessAttestation { outcome, agent, sha256, … }
//! ```
//! Runtimes without [`CAPABILITY`] never poll; the server offers nothing to
//! computers that did not advertise it.
use serde::{Deserialize, Serialize};

/// The runtime can download, verify, install and adopt harness bundles.
pub const CAPABILITY: &str = "rust-harness-update-v1";

/// Signed bytes start with this line, so a harness bundle signature can never
/// be replayed as a runtime release signature (both use the same key).
pub const SIGNING_DOMAIN: &[u8] = b"zuse-harness-bundle-v1\n";

/// Agents whose CLIs ship as harness bundles.
pub const AGENTS: [&str; 3] = ["claude", "codex", "grok"];

/// The adapter contract this source tree speaks to each agent's CLI. A bundle
/// names the contract it was tested against; a runtime adopts only bundles
/// whose contract it speaks. Bump the version when the adapter's expectations
/// of the CLI change incompatibly.
pub fn adapter_for(agent: &str) -> Option<&'static str> {
    match agent {
        "claude" => Some("claude-stream-json-v1"),
        "codex" => Some("codex-app-server-v1"),
        "grok" => Some("acp-v1"),
        _ => None,
    }
}

/// The fields a bundle signature covers. Signed bytes are
/// [`SignedHarnessBundle::signing_bytes`]: [`SIGNING_DOMAIN`] then compact JSON
/// with keys in the order declared here.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SignedHarnessBundle {
    /// Lowercase hex SHA-256 of the uncompressed bundle tar.
    pub sha256: String,
    /// One of [`AGENTS`].
    pub agent: String,
    /// Exact upstream version, e.g. `2.1.312`.
    pub version: String,
    /// `x86_64` or `aarch64`.
    pub architecture: String,
    /// Path of the CLI inside the bundle, e.g. `bin/claude`.
    pub executable: String,
    /// Adapter contract the bundle was tested against ([`adapter_for`]).
    pub adapter: String,
    /// Session-state format this CLI writes.
    pub state_format: String,
    /// Session-state formats this CLI can continue; always includes
    /// `state_format`. A computer adopts a bundle only if it reads the format
    /// the current CLI wrote.
    pub reads_state_formats: Vec<String>,
    /// Auth/protocol capability names the bundle was certified for.
    pub capabilities: Vec<String>,
}

impl SignedHarnessBundle {
    pub fn signing_bytes(&self) -> Vec<u8> {
        let mut bytes = SIGNING_DOMAIN.to_vec();
        bytes.extend(serde_json::to_vec(self).expect("bundle serializes"));
        bytes
    }

    /// Shape checks shared by the server and the computer. Returns a short
    /// reason on failure.
    pub fn validate(&self) -> Result<(), &'static str> {
        if !sha256_valid(&self.sha256) {
            return Err("sha256");
        }
        if !AGENTS.contains(&self.agent.as_str()) {
            return Err("agent");
        }
        if !version_valid(&self.version) {
            return Err("version");
        }
        if !matches!(self.architecture.as_str(), "x86_64" | "aarch64") {
            return Err("architecture");
        }
        if !path_valid(&self.executable) {
            return Err("executable");
        }
        if !name_valid(&self.adapter) || !name_valid(&self.state_format) {
            return Err("format");
        }
        if self.reads_state_formats.is_empty()
            || self.reads_state_formats.len() > 16
            || !self.reads_state_formats.iter().all(|f| name_valid(f))
            || !self.reads_state_formats.contains(&self.state_format)
        {
            return Err("reads_state_formats");
        }
        if self.capabilities.len() > 32 || !self.capabilities.iter().all(|c| name_valid(c)) {
            return Err("capabilities");
        }
        Ok(())
    }

    /// Where the compressed artifact lives in the release bucket.
    pub fn artifact_key(&self) -> String {
        format!(
            "runtime/harness/{}/{}/{}.tar.zst",
            self.agent, self.architecture, self.sha256
        )
    }
}

pub fn sha256_valid(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// 1-64 of `[a-z0-9._-]`, starting with a letter or digit.
pub fn name_valid(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value.as_bytes()[0].is_ascii_alphanumeric()
        && value
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"._-".contains(&b))
}

/// `MAJOR.MINOR.PATCH` with optional `-pre` / `+build` of `[0-9A-Za-z.-]`.
pub fn version_valid(value: &str) -> bool {
    if value.is_empty() || value.len() > 64 {
        return false;
    }
    let (core, rest) = match value.find(['-', '+']) {
        Some(at) => (&value[..at], Some(&value[at + 1..])),
        None => (value, None),
    };
    let parts: Vec<&str> = core.split('.').collect();
    parts.len() == 3
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
        && rest.is_none_or(|r| {
            !r.is_empty()
                && r.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b".-+".contains(&b))
        })
}

/// A relative path of `[A-Za-z0-9._-]` components; no `.`/`..`, at most 256 bytes.
pub fn path_valid(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value.split('/').all(|part| {
            !part.is_empty()
                && part != "."
                && part != ".."
                && part
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
        })
}

/// Where and what to download. `url` is a short-lived presigned GET for the
/// zstd-compressed tar; never trust it without checking `bundle.sha256`
/// (after decompression) and `signature`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HarnessArtifact {
    pub bundle: SignedHarnessBundle,
    /// Standard base64 Ed25519 signature over `bundle.signing_bytes()`.
    pub signature: String,
    pub url: String,
    pub compressed_size_bytes: u64,
    /// Unix ms after which `url` stops working; poll again for a fresh one.
    pub url_expires_at: i64,
}

/// The harness a computer currently binds new processes of one agent to.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HarnessBinding {
    /// Bundle digest, or `None` for the CLI that came with the machine image.
    pub sha256: Option<String>,
    pub version: String,
    pub state_format: String,
}

/// One agent the runtime can run, with the adapter contracts it speaks.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentHarness {
    pub agent: String,
    pub adapters: Vec<String>,
    pub current: Option<HarnessBinding>,
}

/// Body of the runtime's poll.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HarnessPoll {
    pub architecture: String,
    pub agents: Vec<AgentHarness>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HarnessDirective {
    /// Server-minted id; echoed in the attestation.
    pub update_id: String,
    pub artifact: HarnessArtifact,
}

/// Response to the poll: at most one directive per agent.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HarnessOffer {
    pub directives: Vec<HarnessDirective>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum HarnessOutcome {
    /// New processes of this agent now start from the bundle.
    Adopted,
    /// Nothing changed (download, digest, signature, probe or compatibility).
    Failed,
}

/// The computer's report after an attempt. On `adopted`, `sha256`/`version`
/// describe the bundle new processes now use; on `failed`, the bundle that
/// was refused.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HarnessAttestation {
    pub update_id: Option<String>,
    pub outcome: HarnessOutcome,
    pub agent: String,
    pub sha256: String,
    pub version: String,
    pub previous_sha256: Option<String>,
    /// Short `[a-z0-9_-]` code when failed, e.g. `harness_update_artifact_invalid`.
    pub error_code: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bundle() -> SignedHarnessBundle {
        SignedHarnessBundle {
            sha256: "ab".repeat(32),
            agent: "claude".into(),
            version: "2.1.312".into(),
            architecture: "x86_64".into(),
            executable: "bin/claude".into(),
            adapter: "claude-stream-json-v1".into(),
            state_format: "claude-sessions-v1".into(),
            reads_state_formats: vec!["claude-sessions-v1".into()],
            capabilities: vec![],
        }
    }

    #[test]
    fn signing_bytes_are_domain_separated_and_ordered() {
        let text = String::from_utf8(bundle().signing_bytes()).unwrap();
        assert!(text.starts_with("zuse-harness-bundle-v1\n{\"sha256\":"));
        assert!(text.contains("\"agent\":\"claude\",\"version\":\"2.1.312\""));
        assert!(text.ends_with("\"capabilities\":[]}"));
        assert_eq!(
            bundle().artifact_key(),
            format!("runtime/harness/claude/x86_64/{}.tar.zst", "ab".repeat(32))
        );
    }

    /// `infra/cloud/harness-bundle.py` signs the same fixture; both sides
    /// must produce these exact bytes.
    #[test]
    fn signing_bytes_match_the_bundle_pipeline_fixture() {
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../fixtures/harness-bundle-v1.json")).unwrap();
        let bundle: SignedHarnessBundle =
            serde_json::from_value(fixture["bundle"].clone()).unwrap();
        bundle.validate().unwrap();
        assert_eq!(
            String::from_utf8(bundle.signing_bytes()).unwrap(),
            fixture["signingBytes"].as_str().unwrap()
        );
    }

    #[test]
    fn validation_rejects_unsafe_or_inconsistent_bundles() {
        bundle().validate().unwrap();
        type Change = fn(&mut SignedHarnessBundle);
        let cases: Vec<(Change, &str)> = vec![
            (|b| b.agent = "other".into(), "agent"),
            (|b| b.version = "latest".into(), "version"),
            (|b| b.version = "1.2".into(), "version"),
            (|b| b.executable = "../claude".into(), "executable"),
            (|b| b.executable = "/usr/bin/claude".into(), "executable"),
            (|b| b.executable = "bin//claude".into(), "executable"),
            (|b| b.architecture = "arm".into(), "architecture"),
            (
                |b| b.reads_state_formats = vec!["other-v1".into()],
                "reads_state_formats",
            ),
            (|b| b.capabilities = vec!["Bad Name".into()], "capabilities"),
            (|b| b.sha256 = "AB".repeat(32), "sha256"),
        ];
        for (change, reason) in cases {
            let mut b = bundle();
            change(&mut b);
            assert_eq!(b.validate(), Err(reason));
        }
        assert!(version_valid("0.165.0-alpha.2+linux"));
        assert!(path_valid("codex-package/bin/codex"));
    }

    #[test]
    fn every_agent_has_an_adapter_contract() {
        for agent in AGENTS {
            assert!(name_valid(adapter_for(agent).unwrap()));
        }
        assert!(adapter_for("mock").is_none());
    }
}
