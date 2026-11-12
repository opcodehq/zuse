//! Worker-only provider configuration. One missing/restricted provider does not stop the others.
use super::{
    ProviderError,
    boat::Boat,
    boxd::Boxd,
    contract::Provider,
    daytona::{Daytona, SandboxClass},
    e2b::E2b,
    prized::Prized,
};
use std::{collections::BTreeMap, sync::Arc};
use zuse_protocol::cloud_computers::ProviderId;

#[derive(Default)]
pub struct Registry {
    providers: BTreeMap<ProviderId, Arc<dyn Provider>>,
    errors: BTreeMap<ProviderId, &'static str>,
}
impl Registry {
    pub fn from_environment() -> Self {
        Self::configured(|name| std::env::var(name).ok())
    }
    fn configured(env: impl Fn(&str) -> Option<String>) -> Self {
        let mut registry = Self::default();
        for id in ProviderId::ALL {
            let name = match id {
                ProviderId::Boat => "BOAT_API_KEY",
                ProviderId::Boxd => "BOXD_API_KEY",
                ProviderId::E2b => "E2B_API_KEY",
                ProviderId::Daytona => "DAYTONA_API_KEY",
                ProviderId::Prized => "PRIZED_API_KEY",
            };
            let Some(secret) = env(name).filter(|s| !s.is_empty()) else {
                continue;
            };
            let provider: Result<Arc<dyn Provider>, ProviderError> = match id {
                ProviderId::Boat => Boat::new(secret).map(|p| Arc::new(p) as Arc<dyn Provider>),
                ProviderId::Boxd => Boxd::new(secret, env("BOXD_ORG"))
                    .and_then(|p| match env("BOXD_BASE_URL") {
                        Some(base) => p.with_base_url(&base),
                        None => Ok(p),
                    })
                    .map(|p| Arc::new(p) as Arc<dyn Provider>),
                ProviderId::E2b => E2b::new(secret)
                    .and_then(|p| match env("E2B_MAX_TIMEOUT_SECONDS") {
                        Some(raw) => raw
                            .parse()
                            .map_err(|_| ProviderError::Rejected)
                            .and_then(|seconds| p.with_max_timeout(seconds)),
                        None => Ok(p),
                    })
                    .map(|p| Arc::new(p) as Arc<dyn Provider>),
                ProviderId::Prized => Prized::new(secret).map(|p| Arc::new(p) as Arc<dyn Provider>),
                ProviderId::Daytona => {
                    let class = match env("DAYTONA_SANDBOX_CLASS")
                        .as_deref()
                        .unwrap_or("container")
                    {
                        "container" => Some(SandboxClass::Container),
                        "linux-vm" => Some(SandboxClass::LinuxVm),
                        _ => None,
                    };
                    match class {
                        Some(class) => Daytona::new(secret, class, env("DAYTONA_TARGET"))
                            .map(|p| Arc::new(p) as Arc<dyn Provider>),
                        None => Err(ProviderError::Rejected),
                    }
                }
            };
            match provider {
                Ok(provider) => {
                    registry.providers.insert(id, provider);
                }
                Err(_) => {
                    registry
                        .errors
                        .insert(id, "cloud_provider_configuration_invalid");
                }
            }
        }
        registry
    }
    pub fn insert(&mut self, provider: Arc<dyn Provider>) -> Result<(), ProviderError> {
        if self.providers.contains_key(&provider.id()) {
            return Err(ProviderError::Conflict);
        }
        self.providers.insert(provider.id(), provider);
        Ok(())
    }
    pub fn get(&self, id: ProviderId) -> Option<Arc<dyn Provider>> {
        self.providers.get(&id).cloned()
    }
    /// Presence indicates configuration, not successful live certification.
    pub fn configured_ids(&self) -> Vec<ProviderId> {
        self.providers.keys().copied().collect()
    }
    pub fn configuration_errors(&self) -> &BTreeMap<ProviderId, &'static str> {
        &self.errors
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn absent_or_invalid_daytona_configuration_preserves_other_providers() {
        let registry = Registry::configured(|name| match name {
            "BOXD_API_KEY" | "BOAT_API_KEY" | "E2B_API_KEY" | "DAYTONA_API_KEY" => {
                Some("synthetic-private-key".into())
            }
            "DAYTONA_SANDBOX_CLASS" => Some("unapproved-class".into()),
            _ => None,
        });
        for id in [ProviderId::Boxd, ProviderId::Boat, ProviderId::E2b] {
            assert!(registry.get(id).is_some());
        }
        assert!(registry.get(ProviderId::Daytona).is_none());
        assert_eq!(
            registry.configuration_errors().get(&ProviderId::Daytona),
            Some(&"cloud_provider_configuration_invalid")
        );
        assert!(Registry::configured(|_| None).configured_ids().is_empty());
    }
    #[test]
    fn invalid_e2b_timeout_does_not_disable_other_providers() {
        for raw in ["", "unknown", "59", "86401"] {
            let registry = Registry::configured(|name| match name {
                "BOXD_API_KEY" | "BOAT_API_KEY" | "E2B_API_KEY" => Some("fixture-key".into()),
                "E2B_MAX_TIMEOUT_SECONDS" => Some(raw.into()),
                _ => None,
            });
            assert!(registry.get(ProviderId::E2b).is_none());
            assert!(registry.get(ProviderId::Boxd).is_some());
            assert!(registry.get(ProviderId::Boat).is_some());
        }
    }
}
