//! Disabled-unit Boat bases start only after provider hydration and current SQL admission.
use super::*;
use crate::managed_bases::bootstrap::{InitialRuntimeBootstrap, Purpose, Request};
pub(super) fn request(row: &Value, directive: &Directive) -> Result<Option<Request>, ApiError> {
    let Some(spec) = InitialRuntimeBootstrap::from_config(&row["request_config"])? else {
        return Ok(None);
    };
    // Boxd's stopped memory snapshot opt-in does not change retained behavior.
    if row["provider"] != "boat" {
        return Ok(None);
    }
    if !spec.valid(zuse_protocol::cloud_computers::ProviderId::Boat)
        || row["request_config"]["rustProviderControllerVersion"] != 1
        || row["request_config"]["managedReleaseId"]
            .as_str()
            .is_none_or(str::is_empty)
        || directive.action != Action::Resume
        || row["provider_sandbox_id"] != directive.source_machine_id
        || row["request_config"]["runtimeGeneration"] != directive.generation
        || row["request_config"]["rustRuntimeStorage"]["storageIncarnationId"]
            != directive.storage_incarnation_id
    {
        return Err(ApiError::conflict("cloud_lifecycle_fenced"));
    }
    let request = Request {
        version: 1,
        provider: zuse_protocol::cloud_computers::ProviderId::Boat,
        purpose: Purpose::RetainedResume,
        operation_id: directive.operation_id.clone(),
        workspace_id: required(row, "workspace_id")?.into(),
        generation: directive.generation,
        storage_incarnation_id: directive.storage_incarnation_id.clone(),
        source_machine_id: directive.source_machine_id.clone(),
        // A retained disk keeps a runtime swapped in place; expect the
        // computer's committed runtime release, not the base's initial one.
        image_pins: spec.pins_for(&Purpose::RetainedResume, &row["request_config"]),
    };
    if !request.valid() {
        return Err(ApiError::conflict("cloud_lifecycle_fenced"));
    }
    Ok(Some(request))
}

#[cfg(test)]
mod tests;
