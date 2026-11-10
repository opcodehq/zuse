//! Provider metadata for new drafts; task bindings remain authoritative.
use serde_json::Value;

#[derive(Clone, Debug)]
pub(super) struct Provider {
    pub id: String,
    pub reason: Option<String>,
}
impl Provider {
    pub fn ready(&self) -> bool {
        self.reason.is_none()
    }
}
pub(super) fn available(context: &Value) -> Vec<Provider> {
    let mut rows: Vec<_> = context["computerCapabilities"]["providers"].as_array().into_iter().flatten().filter_map(|row| {
        let id=row["id"].as_str()?;
        if !matches!(id,"boxd"|"boat"|"e2b"|"daytona") { return None; }
        let catalog = &context["providerAgentCatalogs"][id];
        let catalog_ready = serde_json::from_value::<zuse_protocol::cloud_catalog::CloudAgentCatalog>(catalog.clone()).ok().is_some_and(|catalog| !catalog.providers.is_empty() && catalog.providers.iter().all(|agent| !agent.models.is_empty() && agent.resolve_model(&agent.default_model).is_some()));
        let ready = serde_json::from_value::<zuse_protocol::cloud_computers::ProviderCapabilities>(row.clone()).ok().is_some_and(|cap|cap.can_create() && cap.command_admission_version==Some(2)) && catalog_ready;
        let reason = (!ready).then(|| row["unavailableReason"].as_str().filter(|s|!s.trim().is_empty()).map(str::to_owned).unwrap_or_else(||format!("{id} is not ready. Choose another computer or ask your administrator to finish its managed environment.")));
        Some(Provider{id:id.into(),reason})
    }).collect();
    rows.sort_by(|a, b| a.id.cmp(&b.id));
    rows.dedup_by(|a, b| a.id == b.id);
    rows
}
pub(super) fn select(
    context: &Value,
    explicit: Option<&str>,
    hint: Option<&str>,
) -> Result<String, String> {
    let rows = available(context);
    if let Some(id) = explicit {
        return rows
            .iter()
            .find(|row| row.id == id)
            .ok_or_else(|| {
                format!("{id} is unavailable for this account. Choose another computer.")
            })
            .and_then(|row| row.reason.clone().map_or_else(|| Ok(row.id.clone()), Err));
    }
    rows.iter().find(|row|Some(row.id.as_str())==hint && row.ready()).or_else(||rows.iter().find(|row|row.ready())).map(|row|row.id.clone()).ok_or_else(||"No cloud computer is ready for this account. Open Environment and connections, or ask your administrator to finish a managed environment.".into())
}
/// A remembered size is a hint only while this provider still advertises it.
/// Never infer a literal `default` for providers whose size IDs differ.
pub(super) fn size(
    context: &Value,
    provider: &str,
    preferred: Option<&str>,
) -> Result<String, String> {
    let unavailable = || {
        format!(
            "{provider} has no available computer sizes. Choose another computer or refresh Cloud setup."
        )
    };
    let row = context["computerCapabilities"]["providers"]
        .as_array()
        .and_then(|rows| rows.iter().find(|row| row["id"] == provider))
        .ok_or_else(unavailable)?;
    let capability =
        serde_json::from_value::<zuse_protocol::cloud_computers::ProviderCapabilities>(row.clone())
            .map_err(|_| unavailable())?;
    if !capability.can_create() {
        return Err(unavailable());
    }
    let sizes: Vec<_> = capability
        .sizes
        .iter()
        .filter(|size| {
            !size.id.trim().is_empty() && size.vcpus > 0 && size.memory_mib > 0 && size.disk_gib > 0
        })
        .collect();
    sizes
        .iter()
        .find(|size| Some(size.id.as_str()) == preferred)
        .or_else(|| sizes.iter().find(|size| size.id == "default"))
        .or_else(|| sizes.first())
        .map(|size| size.id.clone())
        .ok_or_else(unavailable)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn context() -> Value {
        json!({"computerCapabilities":{"providers":[{"id":"boxd","configured":true,"managedBaseReady":false,"commandAdmissionVersion":2,"preservesDisk":true,"preservesMemory":true,"supportsSnapshots":true,"supportsDocker":false,"sizes":[{"id":"default","label":"Standard","vcpus":2,"memoryMib":2048,"diskGib":8}],"unavailableReason":"Boxd is not enabled for this staging account."},{"id":"e2b","configured":true,"managedBaseReady":true,"commandAdmissionVersion":2,"preservesDisk":true,"preservesMemory":true,"supportsSnapshots":true,"supportsDocker":false,"sizes":[{"id":"default","label":"Standard","vcpus":2,"memoryMib":2048,"diskGib":8}],"unavailableReason":null}]},"providerAgentCatalogs":{"e2b":{"providers":[{"id":"codex","name":"Codex","defaultModel":"e2b-model","models":[{"id":"e2b-model","label":"E2B model"}]}]}}})
    }
    #[test]
    fn automatic_default_uses_ready_provider_instead_of_project_hint() {
        assert_eq!(select(&context(), None, Some("boxd")).unwrap(), "e2b");
    }
    #[test]
    fn unavailable_explicit_choice_never_silently_switches() {
        assert!(
            select(&context(), Some("boxd"), Some("e2b"))
                .unwrap_err()
                .contains("not enabled")
        );
        assert!(select(&context(), Some("boat"), None).is_err());
    }
    #[test]
    fn readiness_requires_real_installed_model_catalog() {
        let mut c = context();
        c["providerAgentCatalogs"]["e2b"] = json!({"providers":[]});
        assert!(select(&c, None, None).is_err());
    }
    #[test]
    fn size_selection_uses_supported_preference_or_advertised_default_and_never_invents_a_size() {
        let mut c = context();
        let standard =
            json!({"id":"default","label":"Standard","vcpus":2,"memoryMib":2048,"diskGib":8});
        c["computerCapabilities"]["providers"][1]["sizes"] = json!([{"id":"large","label":"Large","vcpus":4,"memoryMib":8192,"diskGib":16},standard]);
        assert_eq!(size(&c, "e2b", Some("large")).unwrap(), "large");
        assert_eq!(size(&c, "e2b", Some("small")).unwrap(), "default");
        c["computerCapabilities"]["providers"][1]["id"] = json!("boat");
        c["computerCapabilities"]["providers"][1]["sizes"] =
            json!([{"id":"small","label":"Small","vcpus":2,"memoryMib":4096,"diskGib":51}]);
        assert_eq!(size(&c, "boat", Some("default")).unwrap(), "small");
        c["computerCapabilities"]["providers"][1]["sizes"] = json!([]);
        assert!(
            size(&c, "boat", None)
                .unwrap_err()
                .contains("Choose another computer")
        );
        assert!(size(&c, "missing", None).is_err());
    }
}
