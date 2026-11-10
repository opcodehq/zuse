//! Installed agent choices belong to the pinned guest, not a local CLI catalogue.
use super::*;
use std::collections::BTreeMap;
use zuse_protocol::{HarnessId, cloud_catalog::CloudAgentModels};

pub(in crate::runtime) fn agent(harness: &HarnessId) -> Option<&'static str> {
    match harness {
        HarnessId::ClaudeCode => Some("claude"),
        HarnessId::Codex => Some("codex"),
        HarnessId::Grok => Some("grok"),
        _ => None,
    }
}

pub(super) fn installed_catalogs(
    release: &Value,
) -> Result<BTreeMap<String, CloudAgentModels>, ApiError> {
    // An old image may contain three executables but its registry starts only one.
    // Only the exact guest's implementation receipt enables cross-agent admission.
    if release["manifest"]["commandAdmission"]["managedAgentVersion"] != 1 {
        return Ok(BTreeMap::new());
    }
    let mut catalogs = BTreeMap::new();
    for native in release["harnesses"]
        .as_array()
        .ok_or_else(|| error("cloud_task_model_catalog_missing"))?
    {
        let catalog: CloudAgentModels = serde_json::from_value(native["catalog"].clone())
            .map_err(|_| error("cloud_task_model_catalog_missing"))?;
        let name = agent(&catalog.id).ok_or_else(|| error("cloud_task_agent_not_installed"))?;
        if native["agent"] != name
            || catalog.models.is_empty()
            || catalogs.insert(name.into(), catalog).is_some()
        {
            return Err(error("cloud_task_model_catalog_missing"));
        }
    }
    Ok(catalogs)
}

impl Policy {
    pub(in crate::runtime) fn catalog_for(
        &self,
        harness: &HarnessId,
    ) -> Result<&CloudAgentModels, ApiError> {
        if self.catalog.id == *harness {
            return Ok(&self.catalog);
        }
        let name = agent(harness).ok_or_else(|| error("cloud_task_agent_not_installed"))?;
        self.catalogs
            .get(name)
            .filter(|catalog| catalog.id == *harness)
            .ok_or_else(|| error("workspace_agent_switch_update_required"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn catalog(id: HarnessId) -> CloudAgentModels {
        serde_json::from_value(json!({"id":id,"name":"fixture","defaultModel":"fixture-model","models":[{"id":"fixture-model","label":"Fixture","reasoningLevels":[]}]})).unwrap()
    }
    #[test]
    fn old_guest_keeps_its_single_agent_policy() {
        let release = json!({"manifest":{"commandAdmission":{"version":1}},"harnesses":[{"agent":"codex","catalog":catalog(HarnessId::Codex)}]});
        assert!(installed_catalogs(&release).unwrap().is_empty());
    }
    #[test]
    fn attested_guest_uses_only_actual_installed_agent_catalogs() {
        let release = json!({"manifest":{"commandAdmission":{"managedAgentVersion":1}},"harnesses":[{"agent":"codex","catalog":catalog(HarnessId::Codex)},{"agent":"claude","catalog":catalog(HarnessId::ClaudeCode)}]});
        let result = installed_catalogs(&release).unwrap();
        assert_eq!(result.len(), 2);
        assert!(result["claude"].resolve_model("fixture-model").is_some());
        assert!(!result.contains_key("grok"));
        let mut wrong = release;
        wrong["harnesses"][0]["agent"] = json!("grok");
        assert!(installed_catalogs(&wrong).is_err());
    }
}
