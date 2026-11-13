//! Registry of signed guest runtime releases (`zuse_rust_runtime_releases`).
//!
//! Operators register a release CI built and signed; the API only reads the
//! registry to pick each computer's target. Design:
//! `internal-docs/cloud-runtime-updates.md`; wire contract:
//! [`zuse_protocol::cloud_runtime_updates`].
use crate::error::ApiError;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use ed25519_dalek::{Signature, VerifyingKey};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgPool;
use std::cmp::Ordering;
use zuse_protocol::cloud_runtime_updates::{RELEASE_SIGNING_PUBLIC_KEY, SignedRelease};

/// Most capability names a runtime may report. Unknown names are kept (a
/// newer runtime can know more than this server), only their shape is checked.
pub const MAX_CAPABILITIES: usize = 64;

/// A capability or storage-format name: 1-64 of `[a-z0-9._-]`.
pub fn name_valid(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"._-".contains(&b))
}
pub fn names_valid(names: &[String]) -> bool {
    names.len() <= MAX_CAPABILITIES && names.iter().all(|n| name_valid(n))
}
pub(crate) fn sha256_valid(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// `MAJOR.MINOR.PATCH` with an optional `-prerelease` (and ignored `+build`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Version {
    core: [u64; 3],
    pre: Option<String>,
}
impl Version {
    pub fn parse(value: &str) -> Option<Self> {
        if value.is_empty() || value.len() > 64 {
            return None;
        }
        let value = value.split_once('+').map_or(value, |(v, _)| v);
        let (core, pre) = match value.split_once('-') {
            Some((core, pre)) => (core, Some(pre)),
            None => (value, None),
        };
        let mut parts = core.split('.');
        let mut numbers = [0u64; 3];
        for slot in &mut numbers {
            let part = parts.next()?;
            if part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            *slot = part.parse().ok()?;
        }
        if parts.next().is_some()
            || pre.is_some_and(|p| {
                p.is_empty()
                    || !p
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b".-".contains(&b))
            })
        {
            return None;
        }
        Some(Self {
            core: numbers,
            pre: pre.map(str::to_owned),
        })
    }
}
impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        self.core
            .cmp(&other.core)
            .then_with(|| match (&self.pre, &other.pre) {
                (None, None) => Ordering::Equal,
                // A prerelease sorts before its release.
                (Some(_), None) => Ordering::Less,
                (None, Some(_)) => Ordering::Greater,
                (Some(a), Some(b)) => a.cmp(b),
            })
    }
}
impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ReleaseState {
    Candidate,
    Staging,
    Stable,
    Retired,
}
impl ReleaseState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Candidate => "candidate",
            Self::Staging => "staging",
            Self::Stable => "stable",
            Self::Retired => "retired",
        }
    }
    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "candidate" => Self::Candidate,
            "staging" => Self::Staging,
            "stable" => Self::Stable,
            "retired" => Self::Retired,
            _ => return None,
        })
    }
}

/// Which registered releases a deployment moves its computers to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Channel {
    /// Staging deployments take staging and stable releases.
    Staging,
    Stable,
}
impl Channel {
    /// `ZUSE_CLOUD_RUNTIME_CHANNEL`: `staging` or `stable` (default).
    pub fn from_env() -> anyhow::Result<Self> {
        match std::env::var("ZUSE_CLOUD_RUNTIME_CHANNEL").as_deref() {
            Err(_) | Ok("stable") => Ok(Self::Stable),
            Ok("staging") => Ok(Self::Staging),
            Ok(_) => anyhow::bail!("Invalid ZUSE_CLOUD_RUNTIME_CHANNEL"),
        }
    }
    pub fn states(self) -> &'static [&'static str] {
        match self {
            Self::Staging => &["staging", "stable"],
            Self::Stable => &["stable"],
        }
    }
}

/// The production release verifying key from the contract.
pub fn contract_verifying_key() -> VerifyingKey {
    let bytes: [u8; 32] = STANDARD
        .decode(RELEASE_SIGNING_PUBLIC_KEY)
        .ok()
        .and_then(|b| b.try_into().ok())
        .expect("contract release key is 32 bytes");
    VerifyingKey::from_bytes(&bytes).expect("contract release key is a valid point")
}

/// `release.json`, written by the release pipeline next to the artifact.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseFile {
    pub release: SignedRelease,
    /// Standard base64 Ed25519 signature over `release.signing_bytes()`.
    pub signature: String,
    /// The guest's command admission receipt (`runtime-capabilities`),
    /// stored as given.
    pub capabilities: serde_json::Value,
    /// Capability names the release advertises at bootstrap
    /// (`bootstrap-capabilities`). Without it the release satisfies no
    /// capability a message demands.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bootstrap_capabilities: Option<Vec<String>>,
    pub compressed_size_bytes: u64,
    /// Defaults to `runtime/<architecture>/<sha256>.zst`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact_key: Option<String>,
}
impl ReleaseFile {
    pub fn artifact_key(&self) -> String {
        self.artifact_key.clone().unwrap_or_else(|| {
            format!(
                "runtime/{}/{}.zst",
                self.release.architecture, self.release.sha256
            )
        })
    }
    /// Shape checks plus the release signature against `key`.
    pub fn verify(&self, key: &VerifyingKey) -> Result<(), ApiError> {
        let invalid = || ApiError::bad_request("runtime_release_invalid");
        let release = &self.release;
        let key_path = self.artifact_key();
        if !sha256_valid(&release.sha256)
            || Version::parse(&release.version).is_none()
            || !matches!(release.architecture.as_str(), "x86_64" | "aarch64")
            || release.storage_formats.is_empty()
            || release.storage_formats.len() > 16
            || !release.storage_formats.iter().all(|f| name_valid(f))
            || !self.capabilities.is_object()
            || self.capabilities.to_string().len() > 8 * 1024
            || self
                .bootstrap_capabilities
                .as_ref()
                .is_some_and(|names| !names_valid(names))
            || self.compressed_size_bytes == 0
            || self.compressed_size_bytes > 1024 * 1024 * 1024
            || !key_path.starts_with("runtime/")
            || key_path.len() > 512
            || !key_path
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._/-".contains(&b))
            || key_path
                .split('/')
                .any(|s| s.is_empty() || s == "." || s == "..")
        {
            return Err(invalid());
        }
        let signature: [u8; 64] = STANDARD
            .decode(&self.signature)
            .ok()
            .and_then(|b| b.try_into().ok())
            .ok_or_else(|| ApiError::bad_request("runtime_release_signature_invalid"))?;
        key.verify_strict(&release.signing_bytes(), &Signature::from_bytes(&signature))
            .map_err(|_| ApiError::bad_request("runtime_release_signature_invalid"))
    }

    pub async fn register_url(
        &self,
        url: &str,
        key: &VerifyingKey,
        state: ReleaseState,
        now: i64,
    ) -> Result<(), ApiError> {
        self.verify(key)?;
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .connect(url)
            .await
            .map_err(|_| ApiError::unavailable("runtime_release_database_unavailable"))?;
        self.register(&pool, key, state, now).await
    }

    /// Insert the release, or confirm an identical one. A registered release
    /// is immutable apart from its state.
    pub async fn register(
        &self,
        pool: &PgPool,
        key: &VerifyingKey,
        state: ReleaseState,
        now: i64,
    ) -> Result<(), ApiError> {
        self.verify(key)?;
        let release = &self.release;
        let mut tx = pool.begin().await?;
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
            .bind(format!("runtime-release:{}", release.sha256))
            .execute(&mut *tx)
            .await?;
        #[allow(clippy::type_complexity)]
        let prior: Option<(
            String,
            String,
            serde_json::Value,
            serde_json::Value,
            Option<serde_json::Value>,
            String,
            String,
            i64,
        )> =
            sqlx::query_as("SELECT version,architecture,storage_formats,capabilities,bootstrap_capabilities,signature,artifact_key,compressed_size_bytes FROM zuse_rust_runtime_releases WHERE sha256=$1 FOR UPDATE")
                .bind(&release.sha256)
                .fetch_optional(&mut *tx)
                .await?;
        let size = i64::try_from(self.compressed_size_bytes)
            .map_err(|_| ApiError::bad_request("runtime_release_invalid"))?;
        let formats = json!(release.storage_formats);
        let capabilities = self.capabilities.clone();
        let bootstrap = self
            .bootstrap_capabilities
            .as_ref()
            .map(|names| json!(names));
        if let Some(prior) = prior {
            if prior
                != (
                    release.version.clone(),
                    release.architecture.clone(),
                    formats,
                    capabilities,
                    bootstrap.clone(),
                    self.signature.clone(),
                    self.artifact_key(),
                    size,
                )
            {
                return Err(ApiError::conflict("runtime_release_immutable"));
            }
            sqlx::query("UPDATE zuse_rust_runtime_releases SET state=$2,updated_at=$3 WHERE sha256=$1 AND state<>$2")
                .bind(&release.sha256)
                .bind(state.as_str())
                .bind(now)
                .execute(&mut *tx)
                .await?;
        } else {
            sqlx::query("INSERT INTO zuse_rust_runtime_releases(sha256,version,architecture,storage_formats,capabilities,bootstrap_capabilities,signature,artifact_key,compressed_size_bytes,state,created_at,updated_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$11)")
                .bind(&release.sha256)
                .bind(&release.version)
                .bind(&release.architecture)
                .bind(formats)
                .bind(capabilities)
                .bind(bootstrap)
                .bind(&self.signature)
                .bind(self.artifact_key())
                .bind(size)
                .bind(state.as_str())
                .bind(now)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(())
    }
}

/// Move a registered release between channels (or retire it).
pub async fn set_state(
    pool: &PgPool,
    sha256: &str,
    state: ReleaseState,
    now: i64,
) -> Result<(), ApiError> {
    if !sha256_valid(sha256) {
        return Err(ApiError::bad_request("runtime_release_invalid"));
    }
    let changed =
        sqlx::query("UPDATE zuse_rust_runtime_releases SET state=$2,updated_at=$3 WHERE sha256=$1")
            .bind(sha256)
            .bind(state.as_str())
            .bind(now)
            .execute(pool)
            .await?;
    if changed.rows_affected() != 1 {
        return Err(ApiError::not_found("runtime_release_not_found"));
    }
    Ok(())
}
pub async fn set_state_url(
    url: &str,
    sha256: &str,
    state: ReleaseState,
    now: i64,
) -> Result<(), ApiError> {
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect(url)
        .await
        .map_err(|_| ApiError::unavailable("runtime_release_database_unavailable"))?;
    set_state(&pool, sha256, state, now).await
}

/// One registered release, as the API reads it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    pub sha256: String,
    pub version: String,
    pub architecture: String,
    pub storage_formats: Vec<String>,
    /// Bootstrap capability names; empty when the release did not list them.
    pub bootstrap_capabilities: Vec<String>,
    pub signature: String,
    pub artifact_key: String,
    pub compressed_size_bytes: i64,
    pub created_at: i64,
}
impl Release {
    pub fn has(&self, capability: &str) -> bool {
        self.bootstrap_capabilities.iter().any(|c| c == capability)
    }
    pub fn signed(&self) -> SignedRelease {
        SignedRelease {
            sha256: self.sha256.clone(),
            version: self.version.clone(),
            architecture: self.architecture.clone(),
            storage_formats: self.storage_formats.clone(),
        }
    }
}

/// Columns [`Release::from_row`] reads, in order.
pub(crate) const RELEASE_COLUMNS: &str = "sha256,version,architecture,storage_formats,COALESCE(bootstrap_capabilities,'[]'::jsonb),signature,artifact_key,compressed_size_bytes,created_at";
pub(crate) type ReleaseRow = (
    String,
    String,
    String,
    serde_json::Value,
    serde_json::Value,
    String,
    String,
    i64,
    i64,
);
impl Release {
    pub(crate) fn from_row(row: ReleaseRow) -> Self {
        let names = |v: serde_json::Value| serde_json::from_value(v).unwrap_or_default();
        Self {
            sha256: row.0,
            version: row.1,
            architecture: row.2,
            storage_formats: names(row.3),
            bootstrap_capabilities: names(row.4),
            signature: row.5,
            artifact_key: row.6,
            compressed_size_bytes: row.7,
            created_at: row.8,
        }
    }
}

/// The newest channel release, if it is newer than `current`. Releases
/// order by version, then by registration time (CI builds of one version
/// differ only in build metadata). `current` is `None` when the computer's
/// runtime is not in the registry (an initial base runtime registered before
/// the registry existed); any release is then newer. A computer is never
/// offered a downgrade or its own release.
pub fn pick_target(
    current: Option<(&str, &Version, i64)>,
    candidates: Vec<Release>,
) -> Option<Release> {
    candidates
        .into_iter()
        .filter_map(|release| Version::parse(&release.version).map(|v| (v, release)))
        .filter(|(version, release)| match current {
            Some((sha, current, registered)) => {
                release.sha256 != sha && (version, release.created_at) > (current, registered)
            }
            None => true,
        })
        .max_by(|(a, ra), (b, rb)| a.cmp(b).then(ra.created_at.cmp(&rb.created_at)))
        .map(|(_, release)| release)
}

/// A release signed with a throwaway key, for tests.
#[cfg(test)]
pub(crate) fn signed_test_file(
    key: &ed25519_dalek::SigningKey,
    sha: &str,
    version: &str,
) -> ReleaseFile {
    let release = SignedRelease {
        sha256: sha.into(),
        version: version.into(),
        architecture: "x86_64".into(),
        storage_formats: vec!["zuse-rust-engine-v1".into()],
    };
    ReleaseFile {
        signature: STANDARD
            .encode(ed25519_dalek::Signer::sign(key, &release.signing_bytes()).to_bytes()),
        release,
        capabilities: json!({"runtimeSha256": sha}),
        bootstrap_capabilities: Some(vec![
            "rust-rpc-v1".into(),
            "some-future-capability-v9".into(),
        ]),
        compressed_size_bytes: 20 * 1024 * 1024,
        artifact_key: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::SigningKey;

    #[test]
    fn capability_names_tolerate_unknown_names_but_not_bad_shapes() {
        let names = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert!(names_valid(&names(&[
            "rust-rpc-v1",
            "a-name-from-the-future-v7",
            "zots.v1"
        ])));
        assert!(names_valid(&vec!["x".to_owned(); MAX_CAPABILITIES]));
        assert!(!names_valid(&vec!["x".to_owned(); MAX_CAPABILITIES + 1]));
        for bad in ["", "Upper", "has space", "semi;colon", &"a".repeat(65)] {
            assert!(!names_valid(&names(&[bad])), "{bad}");
        }
    }

    #[test]
    fn contract_key_parses() {
        contract_verifying_key();
    }

    #[test]
    fn only_a_release_signed_by_the_release_key_verifies() {
        let signer = SigningKey::from_bytes(&[7; 32]);
        let other = SigningKey::from_bytes(&[8; 32]);
        let file = signed_test_file(&signer, &"a".repeat(64), "1.4.0");
        file.verify(&signer.verifying_key()).unwrap();
        assert_eq!(
            file.artifact_key(),
            format!("runtime/x86_64/{}.zst", "a".repeat(64))
        );
        assert_eq!(
            file.verify(&other.verifying_key()).unwrap_err().code,
            "runtime_release_signature_invalid"
        );
        // The production key never accepts a throwaway signature.
        assert!(file.verify(&contract_verifying_key()).is_err());
        let mut tampered = file.clone();
        tampered.release.version = "9.9.9".into();
        assert_eq!(
            tampered.verify(&signer.verifying_key()).unwrap_err().code,
            "runtime_release_signature_invalid"
        );
        let mut bad = file.clone();
        bad.bootstrap_capabilities = Some(vec!["Not A Name".into()]);
        assert_eq!(
            bad.verify(&signer.verifying_key()).unwrap_err().code,
            "runtime_release_invalid"
        );
        let mut escape = file;
        escape.artifact_key = Some("runtime/../secrets".into());
        assert!(escape.verify(&signer.verifying_key()).is_err());
    }

    #[test]
    fn versions_order_numerically_with_prereleases_first() {
        let v = |s| Version::parse(s).unwrap();
        assert!(v("1.10.0") > v("1.9.9"));
        assert!(v("2.0.0-rc.1") < v("2.0.0"));
        assert!(v("2.0.0+build.5") == v("2.0.0"));
        for bad in ["", "1.2", "1.2.3.4", "v1.2.3", "1.2.x", "1.2.3-"] {
            assert!(Version::parse(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn target_is_the_newest_newer_release_and_never_a_downgrade() {
        let release = |sha: char, version: &str, created_at| Release {
            sha256: sha.to_string().repeat(64),
            version: version.into(),
            architecture: "x86_64".into(),
            storage_formats: vec![],
            bootstrap_capabilities: vec![],
            signature: String::new(),
            artifact_key: String::new(),
            compressed_size_bytes: 1,
            created_at,
        };
        let all = vec![
            release('a', "1.0.0", 1),
            release('b', "1.2.0", 2),
            release('c', "1.1.0", 3),
        ];
        let current = Version::parse("1.1.0").unwrap();
        let sha_c = "c".repeat(64);
        assert_eq!(
            pick_target(Some((&sha_c, &current, 3)), all.clone())
                .unwrap()
                .version,
            "1.2.0"
        );
        let newest = Version::parse("1.2.0").unwrap();
        let sha_b = "b".repeat(64);
        assert!(pick_target(Some((&sha_b, &newest, 2)), all.clone()).is_none());
        let ahead = Version::parse("3.0.0").unwrap();
        assert!(pick_target(Some((&"f".repeat(64), &ahead, 9)), all.clone()).is_none());
        assert_eq!(pick_target(None, all).unwrap().version, "1.2.0");
        // Builds of one version differ only in build metadata: a later
        // registration is newer, an earlier one is not.
        let builds = vec![
            release('d', "0.26.0+gaaaa", 10),
            release('e', "0.26.0+gbbbb", 20),
        ];
        let same = Version::parse("0.26.0+gaaaa").unwrap();
        assert_eq!(
            pick_target(Some((&"d".repeat(64), &same, 10)), builds.clone())
                .unwrap()
                .sha256,
            "e".repeat(64)
        );
        assert!(pick_target(Some((&"e".repeat(64), &same, 20)), builds).is_none());
    }
}
