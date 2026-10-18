//! Device-owned definitions. Credentials stay out of shared workspace documents.
use super::*;
use std::io::Write;
use zuse_protocol::custom_acp::{CustomAcpAgent, CustomAcpAgents};

#[derive(Default)]
pub(super) struct CustomAcpState {
    snapshot: CustomAcpAgents,
    path: Option<PathBuf>,
    error: Option<String>,
}
impl HarnessRegistry {
    pub(super) fn load_custom_acp(&self, root: &Path) {
        let path = root.join("custom-acp-agents.json");
        let loaded = (|| -> Result<CustomAcpAgents, String> {
            match std::fs::metadata(&path) {
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                    return Ok(CustomAcpAgents::default());
                }
                Err(_) => return Err("Custom ACP definitions could not be read.".into()),
                Ok(metadata) if metadata.len() > 4 * 1024 * 1024 => {
                    return Err("Custom ACP definitions exceed the storage limit.".into());
                }
                _ => {}
            }
            let snapshot: CustomAcpAgents = serde_json::from_slice(
                &std::fs::read(&path).map_err(|_| "Custom ACP definitions could not be read.")?,
            )
            .map_err(|_| "Custom ACP definitions are damaged. The file has been preserved.")?;
            validate_snapshot(&snapshot)?;
            Ok(snapshot)
        })();
        let mut state = self
            .custom_acp
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        state.path = Some(path);
        match loaded {
            Ok(snapshot) => {
                for config in &snapshot.agents {
                    // Validation is complete before publishing any configured instance.
                    if let Ok(harness) = zuse_providers::AcpHarness::custom(config.clone()) {
                        self.register(Arc::new(harness));
                    }
                }
                state.snapshot = snapshot;
                state.error = None;
            }
            Err(error) => {
                state.error = Some(error);
            }
        }
    }
    pub fn custom_acp_agents(&self) -> Result<CustomAcpAgents, String> {
        let state = self
            .custom_acp
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some(error) = &state.error {
            return Err(error.clone());
        }
        Ok(state.snapshot.clone())
    }
    pub async fn set_custom_acp_agent(
        &self,
        config: CustomAcpAgent,
        expected_revision: u64,
    ) -> Result<CustomAcpAgents, String> {
        config.validate()?;
        self.change_custom_acp(config.id, Some(config), expected_revision)
            .await
    }
    pub async fn delete_custom_acp_agent(
        &self,
        id: HarnessId,
        expected_revision: u64,
    ) -> Result<CustomAcpAgents, String> {
        if !matches!(id, HarnessId::CustomAcp(_)) {
            return Err("Only a custom ACP definition can be removed.".into());
        }
        self.change_custom_acp(id, None, expected_revision).await
    }
    async fn change_custom_acp(
        &self,
        id: HarnessId,
        config: Option<CustomAcpAgent>,
        expected_revision: u64,
    ) -> Result<CustomAcpAgents, String> {
        let _mutation = self.custom_acp_mutation.lock().await;
        // Existing runs retain their definition. Warm idle processes retire via
        // the same execution gate used for explicit provider updates.
        self.begin_update(id);
        let _intent = Intent { registry: self, id };
        let _exclusive =
            tokio::time::timeout(std::time::Duration::from_secs(15), self.update_lease(id))
                .await
                .map_err(|_| {
                    "This ACP agent is still in use. Stop its sessions and retry.".to_string()
                })?;
        let mut state = self
            .custom_acp
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some(error) = &state.error {
            return Err(error.clone());
        }
        if expected_revision != state.snapshot.revision {
            return Err("Custom ACP definitions changed. Refresh before saving.".into());
        }
        let path = state
            .path
            .clone()
            .ok_or("Custom ACP storage is not configured.")?;
        let mut next = state.snapshot.clone();
        let replacement = if let Some(config) = &config {
            Some(Arc::new(
                zuse_providers::AcpHarness::custom(config.clone())
                    .map_err(|_| "Invalid custom ACP definition.")?,
            ) as Arc<dyn Harness>)
        } else {
            None
        };
        if let Some(config) = config {
            if let Some(existing) = next.agents.iter_mut().find(|agent| agent.id == id) {
                *existing = config;
            } else {
                next.agents.push(config);
            }
        } else {
            if !next.agents.iter().any(|agent| agent.id == id) {
                return Err("Custom ACP definition no longer exists.".into());
            }
            next.agents.retain(|agent| agent.id != id);
        }
        next.revision = next
            .revision
            .checked_add(1)
            .ok_or("Custom ACP revision limit reached.")?;
        validate_snapshot(&next)?;
        write_snapshot(&path, &next)?;
        // No await between durable storage and catalog publication.
        if let Some(harness) = replacement {
            self.register(harness);
        } else {
            self.slots().remove(&id);
            self.order().retain(|entry| *entry != id);
        }
        state.snapshot = next.clone();
        Ok(next)
    }
}
struct Intent<'a> {
    registry: &'a HarnessRegistry,
    id: HarnessId,
}
impl Drop for Intent<'_> {
    fn drop(&mut self) {
        self.registry.end_update(self.id);
    }
}
fn validate_snapshot(snapshot: &CustomAcpAgents) -> Result<(), String> {
    if snapshot.agents.len() > 64 {
        return Err("At most 64 custom ACP agents can be configured on a computer.".into());
    }
    let mut ids = std::collections::HashSet::new();
    for agent in &snapshot.agents {
        agent.validate()?;
        if !ids.insert(agent.id) {
            return Err("Custom ACP identities must be unique.".into());
        }
    }
    Ok(())
}
fn write_snapshot(path: &Path, value: &CustomAcpAgents) -> Result<(), String> {
    let result = (|| -> std::io::Result<()> {
        let parent = path
            .parent()
            .ok_or_else(|| std::io::Error::other("missing profile"))?;
        let bytes = serde_json::to_vec_pretty(value)?;
        if bytes.len() > 4 * 1024 * 1024 {
            return Err(std::io::Error::other("definition storage limit"));
        }
        let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
        temporary.write_all(&bytes)?;
        temporary.as_file().sync_all()?;
        temporary.persist(path).map_err(|err| err.error)?;
        #[cfg(unix)]
        std::fs::File::open(parent)?.sync_all()?;
        Ok(())
    })();
    result.map_err(|_| "Could not confirm saved ACP definitions. Refresh before retrying.".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn agent(id: &str) -> CustomAcpAgent {
        CustomAcpAgent {
            id: HarnessId::try_from(format!("acp-{id}")).unwrap(),
            name: id.into(),
            command: "missing-local-acp".into(),
            args: vec!["stdio".into()],
            env: [("LOCAL_SECRET".into(), "private-test-value".into())]
                .into_iter()
                .collect(),
            model_ids: vec!["local-model".into()],
        }
    }
    #[tokio::test]
    async fn custom_acp_definitions_persist_with_revision_fencing_and_private_permissions() {
        let temp = tempfile::tempdir().unwrap();
        let registry = HarnessRegistry::new();
        registry.load_prefs(temp.path());
        let first = agent("first");
        let second = agent("second");
        assert_eq!(
            registry
                .set_custom_acp_agent(first.clone(), 0)
                .await
                .unwrap()
                .revision,
            1
        );
        assert!(
            registry
                .set_custom_acp_agent(second.clone(), 0)
                .await
                .is_err()
        );
        registry
            .set_custom_acp_agent(second.clone(), 1)
            .await
            .unwrap();
        let public = serde_json::to_string(&registry.descriptors()).unwrap();
        assert!(!public.contains("private-test-value") && !public.contains("LOCAL_SECRET"));
        let reopened = HarnessRegistry::new();
        reopened.load_prefs(temp.path());
        assert_eq!(
            reopened.custom_acp_agents().unwrap().agents,
            vec![first.clone(), second]
        );
        assert_eq!(
            reopened.resolve(first.id).unwrap().fallback_models()[0].id,
            "local-model"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(temp.path().join("custom-acp-agents.json"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
        reopened.delete_custom_acp_agent(first.id, 2).await.unwrap();
        assert!(reopened.resolve(first.id).is_err());
        assert_eq!(reopened.custom_acp_agents().unwrap().revision, 3);
    }
    #[tokio::test]
    async fn malformed_custom_file_is_preserved_and_cancelled_edits_release_the_runtime_gate() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("custom-acp-agents.json");
        std::fs::write(&path, "damaged definition").unwrap();
        let registry = HarnessRegistry::new();
        registry.load_prefs(temp.path());
        assert!(
            registry
                .set_custom_acp_agent(agent("first"), 0)
                .await
                .is_err()
        );
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "damaged definition"
        );
        let other = tempfile::tempdir().unwrap();
        let registry = HarnessRegistry::new();
        registry.load_prefs(other.path());
        let config = agent("first");
        let lease = registry.execution_lease(config.id).await;
        assert!(
            tokio::time::timeout(
                std::time::Duration::from_millis(30),
                registry.set_custom_acp_agent(config.clone(), 0)
            )
            .await
            .is_err()
        );
        assert!(!registry.update_pending(config.id));
        assert!(registry.custom_acp_agents().unwrap().agents.is_empty());
        drop(lease);
        registry.set_custom_acp_agent(config, 0).await.unwrap();
    }
}
