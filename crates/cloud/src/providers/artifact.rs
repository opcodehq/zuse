//! Operational provider version fence; this is not an atomic provider build selector.
use super::ProviderError;
use serde::{Deserialize, Serialize};
use zuse_protocol::cloud_computers::ProviderId;
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ArtifactBinding {
    pub version: u32,
    pub mode: String,
    pub artifact_id: String,
    pub artifact_version: String,
    pub artifact_name: String,
    pub runtime_sha256: String,
    pub manifest_sha256: String,
}
impl ArtifactBinding {
    pub fn validate(&self, provider: ProviderId, artifact: &str) -> Result<(), ProviderError> {
        let identifier = |s: &str| {
            !s.is_empty()
                && s.len() <= 128
                && s.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
        };
        let hash = |s: &str| {
            s.len() == 64
                && s.bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        };
        if provider != ProviderId::E2b
            || self.version != 1
            || self.mode != "private-single-build"
            || self.artifact_id != artifact
            || !identifier(&self.artifact_id)
            || !identifier(&self.artifact_version)
            || !self.artifact_name.starts_with("zuse-e2b-stage-")
            || !identifier(&self.artifact_name)
            || !hash(&self.runtime_sha256)
            || !hash(&self.manifest_sha256)
        {
            return Err(ProviderError::Rejected);
        }
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn binding_is_explicit_provider_and_artifact_scoped() {
        let mut binding = ArtifactBinding {
            version: 1,
            mode: "private-single-build".into(),
            artifact_id: "template-one".into(),
            artifact_version: "build-one".into(),
            artifact_name: "zuse-e2b-stage-one".into(),
            runtime_sha256: "a".repeat(64),
            manifest_sha256: "b".repeat(64),
        };
        assert!(binding.validate(ProviderId::E2b, "template-one").is_ok());
        assert!(binding.validate(ProviderId::Boxd, "template-one").is_err());
        assert!(binding.validate(ProviderId::E2b, "other").is_err());
        binding.manifest_sha256 = "not-a-digest".into();
        assert!(binding.validate(ProviderId::E2b, "template-one").is_err());
    }
}
