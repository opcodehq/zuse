//! Registry of signed agent CLI harness bundles (`zuse_rust_harness_bundles`).
//!
//! Operators register a bundle CI built and signed; the API only reads the
//! registry to pick each computer's target per agent. Operator flow:
//! `infra/cloud/HARNESS_BUNDLES.md`; wire contract:
//! [`zuse_protocol::cloud_harness_updates`].
//!
//! ```text
//! candidate ─▶ staging ─▶ canary ─▶ stable ─▶ retired
//! staging deployment : staging + canary + stable
//! stable deployment  : stable (+ canary for ZUSE_CLOUD_HARNESS_CANARY_ACCOUNTS)
//! ```
use crate::error::ApiError;
use crate::runtime_releases::Version;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use ed25519_dalek::{Signature, VerifyingKey};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgPool;
use std::collections::BTreeSet;
use zuse_protocol::cloud_harness_updates::{HarnessBinding, SignedHarnessBundle};

/// After this many failed attempts a bundle is not offered to that computer.
pub const MAX_FAILURES: i32 = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BundleState {
    Candidate,
    Staging,
    Canary,
    Stable,
    Retired,
}
impl BundleState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Candidate => "candidate",
            Self::Staging => "staging",
            Self::Canary => "canary",
            Self::Stable => "stable",
            Self::Retired => "retired",
        }
    }
    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "candidate" => Self::Candidate,
            "staging" => Self::Staging,
            "canary" => Self::Canary,
            "stable" => Self::Stable,
            "retired" => Self::Retired,
            _ => return None,
        })
    }
}

/// Which registered bundles a deployment moves its computers to.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Channel {
    pub staging: bool,
    /// Accounts that also take `canary` bundles on a stable deployment.
    pub canary_accounts: BTreeSet<String>,
}
impl Channel {
    /// `ZUSE_CLOUD_HARNESS_CHANNEL` (`staging` or `stable`, default) and
    /// `ZUSE_CLOUD_HARNESS_CANARY_ACCOUNTS` (comma-separated account ids).
    pub fn from_env() -> anyhow::Result<Self> {
        let staging = match std::env::var("ZUSE_CLOUD_HARNESS_CHANNEL").as_deref() {
            Err(_) | Ok("stable") => false,
            Ok("staging") => true,
            Ok(_) => anyhow::bail!("Invalid ZUSE_CLOUD_HARNESS_CHANNEL"),
        };
        let canary_accounts = std::env::var("ZUSE_CLOUD_HARNESS_CANARY_ACCOUNTS")
            .unwrap_or_default()
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .collect();
        Ok(Self {
            staging,
            canary_accounts,
        })
    }
    /// Bundle states offered to a computer owned by `account`.
    pub fn states(&self, account: Option<&str>) -> Vec<&'static str> {
        if self.staging {
            vec!["staging", "canary", "stable"]
        } else if account.is_some_and(|a| self.canary_accounts.contains(a)) {
            vec!["canary", "stable"]
        } else {
            vec!["stable"]
        }
    }
}

/// `bundle.json`, written by `infra/cloud/harness-bundle.py`. Unsigned build
/// evidence (`files`, `upstream`) is accepted and not stored.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BundleFile {
    pub bundle: SignedHarnessBundle,
    /// Standard base64 Ed25519 signature over `bundle.signing_bytes()`.
    pub signature: String,
    pub compressed_size_bytes: u64,
    /// Defaults to [`SignedHarnessBundle::artifact_key`]; must equal it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact_key: Option<String>,
}
impl BundleFile {
    pub fn artifact_key(&self) -> String {
        self.artifact_key
            .clone()
            .unwrap_or_else(|| self.bundle.artifact_key())
    }
    /// Shape checks plus the bundle signature against `key`.
    pub fn verify(&self, key: &VerifyingKey) -> Result<(), ApiError> {
        if self.bundle.validate().is_err()
            || self.artifact_key() != self.bundle.artifact_key()
            || self.compressed_size_bytes == 0
            || self.compressed_size_bytes > 1024 * 1024 * 1024
        {
            return Err(ApiError::bad_request("harness_bundle_invalid"));
        }
        let signature: [u8; 64] = STANDARD
            .decode(&self.signature)
            .ok()
            .and_then(|b| b.try_into().ok())
            .ok_or_else(|| ApiError::bad_request("harness_bundle_signature_invalid"))?;
        key.verify_strict(
            &self.bundle.signing_bytes(),
            &Signature::from_bytes(&signature),
        )
        .map_err(|_| ApiError::bad_request("harness_bundle_signature_invalid"))
    }

    pub async fn register_url(
        &self,
        url: &str,
        key: &VerifyingKey,
        state: BundleState,
        now: i64,
    ) -> Result<(), ApiError> {
        self.verify(key)?;
        let pool = connect(url).await?;
        self.register(&pool, key, state, now).await
    }

    /// Insert the bundle, or confirm an identical one. A registered bundle is
    /// immutable apart from its state.
    pub async fn register(
        &self,
        pool: &PgPool,
        key: &VerifyingKey,
        state: BundleState,
        now: i64,
    ) -> Result<(), ApiError> {
        self.verify(key)?;
        let b = &self.bundle;
        let size = i64::try_from(self.compressed_size_bytes)
            .map_err(|_| ApiError::bad_request("harness_bundle_invalid"))?;
        let mut tx = pool.begin().await?;
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
            .bind(format!("harness-bundle:{}", b.sha256))
            .execute(&mut *tx)
            .await?;
        let prior: Option<serde_json::Value> = sqlx::query_scalar("SELECT jsonb_build_array(agent,version,architecture,executable,adapter,state_format,reads_state_formats,capabilities,signature,artifact_key,compressed_size_bytes) FROM zuse_rust_harness_bundles WHERE sha256=$1 FOR UPDATE")
            .bind(&b.sha256)
            .fetch_optional(&mut *tx)
            .await?;
        let record = json!([
            b.agent,
            b.version,
            b.architecture,
            b.executable,
            b.adapter,
            b.state_format,
            b.reads_state_formats,
            b.capabilities,
            self.signature,
            self.artifact_key(),
            size
        ]);
        if let Some(prior) = prior {
            if prior != record {
                return Err(ApiError::conflict("harness_bundle_immutable"));
            }
            sqlx::query("UPDATE zuse_rust_harness_bundles SET state=$2,updated_at=$3 WHERE sha256=$1 AND state<>$2")
                .bind(&b.sha256)
                .bind(state.as_str())
                .bind(now)
                .execute(&mut *tx)
                .await?;
        } else {
            sqlx::query("INSERT INTO zuse_rust_harness_bundles(sha256,agent,version,architecture,executable,adapter,state_format,reads_state_formats,capabilities,signature,artifact_key,compressed_size_bytes,state,created_at,updated_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$14)")
                .bind(&b.sha256)
                .bind(&b.agent)
                .bind(&b.version)
                .bind(&b.architecture)
                .bind(&b.executable)
                .bind(&b.adapter)
                .bind(&b.state_format)
                .bind(json!(b.reads_state_formats))
                .bind(json!(b.capabilities))
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

async fn connect(url: &str) -> Result<PgPool, ApiError> {
    sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect(url)
        .await
        .map_err(|_| ApiError::unavailable("harness_bundle_database_unavailable"))
}

/// Move a registered bundle between channels (or retire it).
pub async fn set_state(
    pool: &PgPool,
    sha256: &str,
    state: BundleState,
    now: i64,
) -> Result<(), ApiError> {
    if !zuse_protocol::cloud_harness_updates::sha256_valid(sha256) {
        return Err(ApiError::bad_request("harness_bundle_invalid"));
    }
    let changed =
        sqlx::query("UPDATE zuse_rust_harness_bundles SET state=$2,updated_at=$3 WHERE sha256=$1")
            .bind(sha256)
            .bind(state.as_str())
            .bind(now)
            .execute(pool)
            .await?;
    if changed.rows_affected() != 1 {
        return Err(ApiError::not_found("harness_bundle_not_found"));
    }
    Ok(())
}
pub async fn set_state_url(
    url: &str,
    sha256: &str,
    state: BundleState,
    now: i64,
) -> Result<(), ApiError> {
    set_state(&connect(url).await?, sha256, state, now).await
}

/// One registered bundle, as the API reads it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bundle {
    pub signed: SignedHarnessBundle,
    pub signature: String,
    pub artifact_key: String,
    pub compressed_size_bytes: i64,
    pub state: String,
    pub created_at: i64,
}

/// Columns [`Bundle::from_row`] reads, in order.
pub(crate) const BUNDLE_COLUMNS: &str = "sha256,agent,version,architecture,executable,adapter,state_format,reads_state_formats,capabilities,signature,artifact_key,compressed_size_bytes,state,created_at";
pub(crate) type BundleRow = (
    String,
    String,
    String,
    String,
    String,
    String,
    String,
    serde_json::Value,
    serde_json::Value,
    String,
    String,
    i64,
    String,
    i64,
);
impl Bundle {
    pub(crate) fn from_row(row: BundleRow) -> Self {
        let names = |v: serde_json::Value| serde_json::from_value(v).unwrap_or_default();
        Self {
            signed: SignedHarnessBundle {
                sha256: row.0,
                agent: row.1,
                version: row.2,
                architecture: row.3,
                executable: row.4,
                adapter: row.5,
                state_format: row.6,
                reads_state_formats: names(row.7),
                capabilities: names(row.8),
            },
            signature: row.9,
            artifact_key: row.10,
            compressed_size_bytes: row.11,
            state: row.12,
            created_at: row.13,
        }
    }
}

/// What the computer runs now for one agent, joined with the registry.
#[derive(Clone, Debug)]
pub struct Current<'a> {
    pub binding: &'a HarnessBinding,
    /// The registry row of `binding.sha256`, when registered.
    pub registered: Option<&'a Bundle>,
}

/// The bundle one agent on one computer should move to, if any.
///
/// `candidates` are the channel's bundles for this agent and architecture.
/// A bundle qualifies when the runtime speaks its adapter contract, it can
/// continue the session state the current CLI wrote, and it has not failed
/// [`MAX_FAILURES`] times here (`failed`). Normally only a newer bundle is
/// offered (by version, then registration time). When the current bundle was
/// retired, the newest qualifying bundle is offered even if older: that is
/// how an operator rolls a bad CLI back.
pub fn pick_target(
    current: Option<Current<'_>>,
    adapters: &[String],
    candidates: Vec<Bundle>,
    failed: &BTreeSet<String>,
) -> Option<Bundle> {
    let current_sha = current.as_ref().and_then(|c| c.binding.sha256.clone());
    let retired = current
        .as_ref()
        .and_then(|c| c.registered)
        .is_some_and(|r| r.state == "retired");
    let floor = current.as_ref().and_then(|c| {
        Version::parse(&c.binding.version)
            .map(|v| (v, c.registered.map_or(i64::MAX, |r| r.created_at)))
    });
    candidates
        .into_iter()
        .filter(|b| {
            Some(&b.signed.sha256) != current_sha.as_ref()
                && b.state != "retired"
                && adapters.contains(&b.signed.adapter)
                && !failed.contains(&b.signed.sha256)
                && current.as_ref().is_none_or(|c| {
                    b.signed
                        .reads_state_formats
                        .contains(&c.binding.state_format)
                })
        })
        .filter_map(|b| Version::parse(&b.signed.version).map(|v| (v, b)))
        .filter(|(version, bundle)| {
            if retired {
                return true;
            }
            match &floor {
                // An unregistered (image) CLI never yields to a same-version bundle.
                Some((current, registered)) => {
                    version > current || (version == current && bundle.created_at > *registered)
                }
                None => current.is_none(),
            }
        })
        .max_by(|(a, ra), (b, rb)| a.cmp(b).then(ra.created_at.cmp(&rb.created_at)))
        .map(|(_, bundle)| bundle)
}

/// A bundle signed with a throwaway key, for tests.
#[cfg(test)]
pub(crate) fn signed_test_file(
    key: &ed25519_dalek::SigningKey,
    agent: &str,
    sha: &str,
    version: &str,
) -> BundleFile {
    let bundle = SignedHarnessBundle {
        sha256: sha.into(),
        agent: agent.into(),
        version: version.into(),
        architecture: "x86_64".into(),
        executable: format!("bin/{agent}"),
        adapter: zuse_protocol::cloud_harness_updates::adapter_for(agent)
            .unwrap()
            .into(),
        state_format: format!("{agent}-sessions-v1"),
        reads_state_formats: vec![format!("{agent}-sessions-v1"), "image-tested-v1".into()],
        capabilities: vec![],
    };
    BundleFile {
        signature: STANDARD
            .encode(ed25519_dalek::Signer::sign(key, &bundle.signing_bytes()).to_bytes()),
        bundle,
        compressed_size_bytes: 40 * 1024 * 1024,
        artifact_key: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::SigningKey;

    #[test]
    fn only_a_bundle_signed_by_the_release_key_verifies() {
        let signer = SigningKey::from_bytes(&[17; 32]);
        let file = signed_test_file(&signer, "claude", &"a".repeat(64), "2.1.312");
        file.verify(&signer.verifying_key()).unwrap();
        assert_eq!(
            file.verify(&SigningKey::from_bytes(&[18; 32]).verifying_key())
                .unwrap_err()
                .code,
            "harness_bundle_signature_invalid"
        );
        assert!(
            file.verify(&crate::runtime_releases::contract_verifying_key())
                .is_err()
        );
        let mut tampered = file.clone();
        tampered.bundle.reads_state_formats.push("other-v1".into());
        assert_eq!(
            tampered.verify(&signer.verifying_key()).unwrap_err().code,
            "harness_bundle_signature_invalid"
        );
        let mut moved = file.clone();
        moved.artifact_key = Some(format!("runtime/x86_64/{}.zst", "a".repeat(64)));
        assert_eq!(
            moved.verify(&signer.verifying_key()).unwrap_err().code,
            "harness_bundle_invalid"
        );
        let mut unsafe_path = file;
        unsafe_path.bundle.executable = "../bin/claude".into();
        assert_eq!(
            unsafe_path
                .verify(&signer.verifying_key())
                .unwrap_err()
                .code,
            "harness_bundle_invalid"
        );
    }

    /// The pipeline fixture verifies with its throwaway key, and a runtime
    /// release signature over the same key is never a bundle signature.
    #[test]
    fn pipeline_fixture_signature_verifies() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../protocol/fixtures/harness-bundle-v1.json"
        ))
        .unwrap();
        let key: [u8; 32] = STANDARD
            .decode(fixture["testPublicKey"].as_str().unwrap())
            .unwrap()
            .try_into()
            .unwrap();
        let file = BundleFile {
            bundle: serde_json::from_value(fixture["bundle"].clone()).unwrap(),
            signature: fixture["testSignature"].as_str().unwrap().into(),
            compressed_size_bytes: 1,
            artifact_key: None,
        };
        file.verify(&VerifyingKey::from_bytes(&key).unwrap())
            .unwrap();
    }

    fn bundle(sha: char, version: &str, created_at: i64, state: &str) -> Bundle {
        let file = signed_test_file(
            &SigningKey::from_bytes(&[1; 32]),
            "codex",
            &sha.to_string().repeat(64),
            version,
        );
        Bundle {
            signed: file.bundle,
            signature: file.signature,
            artifact_key: String::new(),
            compressed_size_bytes: 1,
            state: state.into(),
            created_at,
        }
    }

    #[test]
    fn target_is_newest_compatible_newer_bundle_with_rollback_on_retire() {
        let adapters = vec!["codex-app-server-v1".to_owned()];
        let none = BTreeSet::new();
        let all = vec![
            bundle('a', "0.165.0", 1, "stable"),
            bundle('b', "0.166.0", 2, "stable"),
            bundle('c', "0.167.0", 3, "stable"),
        ];
        // The image's CLI: no sha, known state format.
        let image = HarnessBinding {
            sha256: None,
            version: "0.165.0".into(),
            state_format: "image-tested-v1".into(),
        };
        let current = Some(Current {
            binding: &image,
            registered: None,
        });
        let pick = |c: Option<Current<'_>>, list: Vec<Bundle>, failed: &BTreeSet<String>| {
            pick_target(c, &adapters, list, failed).map(|b| b.signed.version)
        };
        assert_eq!(
            pick(current.clone(), all.clone(), &none).as_deref(),
            Some("0.167.0")
        );
        // A bundle that failed here twice is skipped.
        let failed: BTreeSet<String> = ["c".repeat(64)].into();
        assert_eq!(
            pick(current.clone(), all.clone(), &failed).as_deref(),
            Some("0.166.0")
        );
        // A runtime that does not speak the bundle's adapter gets nothing.
        assert!(pick_target(current.clone(), &["acp-v1".into()], all.clone(), &none).is_none());
        // A CLI whose session state the bundles cannot continue stays pinned.
        let foreign = HarnessBinding {
            state_format: "codex-sessions-v9".into(),
            ..image.clone()
        };
        assert!(
            pick(
                Some(Current {
                    binding: &foreign,
                    registered: None
                }),
                all.clone(),
                &none
            )
            .is_none()
        );
        // Never a downgrade or the same version while the current is good.
        let on_c = all[2].clone();
        let binding = HarnessBinding {
            sha256: Some("c".repeat(64)),
            version: "0.167.0".into(),
            state_format: "codex-sessions-v1".into(),
        };
        let current_c = || {
            Some(Current {
                binding: &binding,
                registered: Some(&on_c),
            })
        };
        assert!(pick(current_c(), all.clone(), &none).is_none());
        // Retiring the current bundle rolls back to the newest good one.
        let mut retired = on_c.clone();
        retired.state = "retired".into();
        let mut list = all.clone();
        list[2].state = "retired".into();
        assert_eq!(
            pick(
                Some(Current {
                    binding: &binding,
                    registered: Some(&retired)
                }),
                list,
                &none
            )
            .as_deref(),
            Some("0.166.0")
        );
        // An image CLI never yields to a same-version bundle; a later build
        // of a registered version does.
        let same = vec![bundle('d', "0.165.0", 9, "stable")];
        assert!(pick(current.clone(), same, &none).is_none());
        let first = bundle('e', "0.168.0", 10, "stable");
        let rebuild = vec![first.clone(), bundle('f', "0.168.0", 11, "stable")];
        let on_e = HarnessBinding {
            sha256: Some("e".repeat(64)),
            version: "0.168.0".into(),
            state_format: "codex-sessions-v1".into(),
        };
        assert_eq!(
            pick_target(
                Some(Current {
                    binding: &on_e,
                    registered: Some(&first)
                }),
                &adapters,
                rebuild,
                &none
            )
            .unwrap()
            .signed
            .sha256,
            "f".repeat(64)
        );
        // Nothing known about the computer: the newest bundle.
        assert_eq!(pick(None, all, &none).as_deref(), Some("0.167.0"));
    }

    #[test]
    fn channels_widen_for_staging_and_canary_accounts() {
        let mut channel = Channel::default();
        assert_eq!(channel.states(Some("a")), vec!["stable"]);
        channel.canary_accounts.insert("a".into());
        assert_eq!(channel.states(Some("a")), vec!["canary", "stable"]);
        assert_eq!(channel.states(Some("b")), vec!["stable"]);
        channel.staging = true;
        assert_eq!(channel.states(None), vec!["staging", "canary", "stable"]);
        for state in ["candidate", "staging", "canary", "stable", "retired"] {
            assert_eq!(BundleState::parse(state).unwrap().as_str(), state);
        }
        assert!(BundleState::parse("latest").is_none());
    }
}
