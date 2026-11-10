//! Cache identity contains reproducible inputs, never a task or agent connection.
use crate::error::ApiError;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use zuse_protocol::cloud_computers::ProviderId;

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CacheIdentity {
    pub version: u32,
    pub owner_id: String,
    pub project_id: String,
    pub repository_identity: String,
    pub provider: ProviderId,
    pub architecture: String,
    pub base_release_id: String,
    pub base_artifact_id: String,
    pub base_artifact_version: Option<String>,
    pub base_runtime_sha256: String,
    pub recipe_source: String,
    pub recipe_digest: String,
    /// Exact commit is the conservative default: custom setup can read any file.
    pub source_commit: String,
    pub dependency_inputs: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub staging_pilot_sha256: Option<String>,
}
fn digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}
fn id(value: &str) -> bool {
    !value.is_empty() && value.len() <= 512 && !value.chars().any(char::is_control)
}
impl CacheIdentity {
    pub fn validate(&self) -> Result<(), ApiError> {
        if self
            .staging_pilot_sha256
            .as_deref()
            .is_some_and(|value| !digest(value))
            || self.version != 1
            || !id(&self.owner_id)
            || !id(&self.project_id)
            || !id(&self.repository_identity)
            || !id(&self.base_release_id)
            || !id(&self.base_artifact_id)
            || !matches!(self.architecture.as_str(), "x86_64" | "aarch64")
            || !matches!(self.recipe_source.as_str(), "project" | "repository")
            || !digest(&self.base_runtime_sha256)
            || !digest(&self.recipe_digest)
            || !matches!(self.source_commit.len(), 40 | 64)
            || !self
                .source_commit
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
            || self
                .base_artifact_version
                .as_deref()
                .is_some_and(|s| !id(s))
            || self.dependency_inputs.len() > 128
            || self.dependency_inputs.iter().any(|(path, hash)| {
                path.is_empty()
                    || path.len() > 1024
                    || path.starts_with('/')
                    || path
                        .split('/')
                        .any(|p| p == ".." || p == "." || p.is_empty())
                    || path.contains('\\')
                    || !matches!(hash.len(), 40 | 64)
                    || !hash
                        .bytes()
                        .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
            })
        {
            return Err(ApiError::bad_request("project_cache_identity_invalid"));
        }
        Ok(())
    }
    pub fn key(&self) -> Result<String, ApiError> {
        self.validate()?;
        Ok(format!(
            "{:x}",
            Sha256::digest(
                serde_json::to_vec(self)
                    .map_err(|_| ApiError::bad_request("project_cache_identity_invalid"))?
            )
        ))
    }
    pub fn dependency_digest(&self) -> String {
        format!(
            "{:x}",
            Sha256::digest(
                serde_json::to_vec(&(&self.source_commit, &self.dependency_inputs))
                    .expect("dependency pin serializes")
            )
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn identity() -> CacheIdentity {
        CacheIdentity {
            version: 1,
            owner_id: "owner".into(),
            project_id: "project".into(),
            repository_identity: "github.com/example/project".into(),
            provider: ProviderId::Boxd,
            architecture: "x86_64".into(),
            base_release_id: "base-v1".into(),
            base_artifact_id: "base-snapshot".into(),
            base_artifact_version: None,
            base_runtime_sha256: "a".repeat(64),
            recipe_source: "project".into(),
            recipe_digest: "b".repeat(64),
            source_commit: "c".repeat(40),
            dependency_inputs: BTreeMap::from([("package-lock.json".into(), "d".repeat(40))]),
            staging_pilot_sha256: None,
        }
    }
    #[test]
    fn identity_is_scoped_and_has_no_connection_or_credential_inputs() {
        let original = identity();
        let key = original.key().unwrap();
        for changed in [
            CacheIdentity {
                owner_id: "other".into(),
                ..original.clone()
            },
            CacheIdentity {
                project_id: "other".into(),
                ..original.clone()
            },
            CacheIdentity {
                provider: ProviderId::Boat,
                ..original.clone()
            },
            CacheIdentity {
                architecture: "aarch64".into(),
                ..original.clone()
            },
            CacheIdentity {
                base_release_id: "base-v2".into(),
                ..original.clone()
            },
            CacheIdentity {
                recipe_digest: "e".repeat(64),
                ..original.clone()
            },
            CacheIdentity {
                source_commit: "f".repeat(40),
                ..original.clone()
            },
        ] {
            assert_ne!(key, changed.key().unwrap());
        }
        let encoded = serde_json::to_value(original).unwrap();
        assert!(encoded.get("agentConnectionId").is_none());
        assert!(encoded.get("secretBindings").is_none());
    }
    #[test]
    fn legacy_identity_key_is_unchanged_and_each_pilot_has_a_separate_namespace() {
        let mut identity = identity();
        assert_eq!(
            identity.key().unwrap(),
            "7b9bf317cb53ad5b61835a0c83c4996a6b4ce94b1bf9f469018502f20d25d661"
        );
        assert!(
            serde_json::to_value(&identity)
                .unwrap()
                .get("stagingPilotSha256")
                .is_none()
        );
        let normal = identity.key().unwrap();
        identity.staging_pilot_sha256 = Some("a".repeat(64));
        let first = identity.key().unwrap();
        assert_ne!(normal, first);
        identity.staging_pilot_sha256 = Some("b".repeat(64));
        assert_ne!(first, identity.key().unwrap());
    }
    #[test]
    fn dependency_paths_cannot_reference_host_files() {
        let mut i = identity();
        i.dependency_inputs
            .insert("../credentials".into(), "a".repeat(40));
        assert!(i.validate().is_err());
    }
}
