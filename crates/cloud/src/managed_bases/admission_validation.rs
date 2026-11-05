//! Pure validation of rows obtained through the registry admission lock.
use super::*;
pub(crate) fn source_from_locked_records(
    records: &[Value],
    provider: &str,
    agent: &str,
    model: &str,
    size: &str,
    organization: bool,
) -> Result<Value, ApiError> {
    let agent = if agent == "claude-code" {
        "claude"
    } else {
        agent
    };
    let mut supervisor_missing = false;
    let mut organization_missing = false;
    for row in records {
        let id = row["release_id"]
            .as_str()
            .ok_or_else(|| ApiError::unavailable("managed_base_manifest_invalid"))?;
        if row["state"] != "verified" || row["runtime_manifest"].is_null() {
            continue;
        }
        let artifact = &row["artifact_id"];
        let architecture = &row["architecture"];
        let capabilities = &row["capabilities"];
        let release: ManagedBaseRelease =
            serde_json::from_value(row["runtime_manifest"].clone())
                .map_err(|_| ApiError::unavailable("managed_base_manifest_invalid"))?;
        release
            .validate()
            .map_err(|_| ApiError::unavailable("managed_base_manifest_invalid"))?;
        if release.provider.as_str() != provider
            || release.release_id != id
            || json!(release.artifact_id) != *artifact
            || json!(release.architecture) != *architecture
            || json!(release.capabilities) != *capabilities
        {
            return Err(ApiError::unavailable("managed_base_manifest_invalid"));
        }
        let eligible = release
            .harnesses
            .iter()
            .any(|h| h.agent.as_str() == agent && h.catalog.resolve_model(model).is_some())
            && release.capabilities.sizes.iter().any(|s| s.id == size);
        if eligible {
            if release.provider == ProviderId::Boxd
                && release.manifest.rust_runtime_supervisor_version != Some(1)
            {
                supervisor_missing = true;
                continue;
            }
            if organization && release.manifest.rust_organization_execution_version != Some(1) {
                organization_missing = true;
                continue;
            }
            return Ok(
                json!({"build_id":null,"snapshot_id":release.artifact_id,"settings":release.settings()}),
            );
        }
    }
    Err(ApiError::conflict(if supervisor_missing {
        "managed_base_runtime_supervisor_required"
    } else if organization_missing {
        "managed_base_organization_runtime_required"
    } else {
        "managed_base_unavailable_for_agent"
    }))
}
