//! Installed harnesses share one engine, with credentials requested only on use.
use super::*;
use zuse_protocol::HarnessId;

pub struct ProviderBrokers {
    brokers: Vec<Arc<ProviderBroker>>,
}
fn provider(harness: HarnessId) -> Option<CloudAuthProvider> {
    match harness {
        HarnessId::ClaudeCode => Some(CloudAuthProvider::Claude),
        HarnessId::Codex => Some(CloudAuthProvider::Codex),
        HarnessId::Grok => Some(CloudAuthProvider::Grok),
        _ => None,
    }
}
impl ProviderBrokers {
    pub fn new(brokers: Vec<Arc<ProviderBroker>>) -> Self {
        Self { brokers }
    }
    pub fn codex(&self) -> Option<&ProviderBroker> {
        self.brokers
            .iter()
            .find(|broker| broker.provider == CloudAuthProvider::Codex)
            .map(Arc::as_ref)
    }
    pub async fn resume_retained(
        &self,
        core: &zuse_engine::EngineCore,
        boot: &CloudRuntimeBootstrap,
    ) -> Result<()> {
        let mut needed = Vec::new();
        for id in crate::lifecycle::family(core, boot)? {
            if let Some(selected) = core.sessions.live_harness(&id).and_then(provider)
                && !needed.contains(&selected)
            {
                needed.push(selected);
            }
        }
        for selected in needed {
            let broker = self
                .brokers
                .iter()
                .find(|broker| broker.provider == selected)
                .ok_or_else(|| anyhow::anyhow!("runtime_agent_not_installed"))?;
            broker.resume_credential().await?;
        }
        Ok(())
    }
}
