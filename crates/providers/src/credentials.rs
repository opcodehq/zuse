//! Ephemeral credentials supplied by the host, outside serialized run requests and history.
pub mod redaction;
use crate::{HarnessError, process::Command};
use async_trait::async_trait;
use zeroize::Zeroizing;
pub struct ChildEnvironment {
    values: Vec<(String, Zeroizing<String>)>,
}
impl ChildEnvironment {
    pub fn empty() -> Self {
        Self { values: Vec::new() }
    }
    pub fn new(key: &'static str, value: Zeroizing<String>) -> Self {
        Self {
            values: vec![(key.into(), value)],
        }
    }
    pub fn and(mut self, key: &'static str, value: Zeroizing<String>) -> Self {
        self.values.push((key.into(), value));
        self
    }
    /// Workload values precede selected agent credentials, which retain priority.
    pub fn with_workload(
        mut self,
        values: impl IntoIterator<Item = (String, Zeroizing<String>)>,
    ) -> Self {
        let mut workload = values.into_iter().collect::<Vec<_>>();
        workload.extend(self.values);
        self.values = workload;
        self
    }
    pub(crate) fn external_grok_auth(&self) -> bool {
        self.values
            .iter()
            .any(|(key, _)| key == "GROK_AUTH_PROVIDER_COMMAND")
    }
    pub fn apply(&self, command: &mut Command) {
        // Brokered processes must never fall back to another inherited account.
        command
            .env_remove("ANTHROPIC_API_KEY")
            .env_remove("CLAUDE_CODE_OAUTH_TOKEN")
            .env_remove("ANTHROPIC_AUTH_TOKEN")
            .env_remove("OPENAI_API_KEY")
            .env_remove("CODEX_API_KEY")
            .env_remove("CODEX_ACCESS_TOKEN")
            .env_remove("CLOUD_DATA_ENCRYPTION_KEY")
            .env_remove("CLOUD_CREDENTIAL_VAULT_KEY")
            .env_remove("RELAY_MINT_PRIVATE_JWK")
            .env_remove("WORKOS_API_KEY")
            .env_remove("XAI_API_KEY")
            .env_remove("GROK_API_KEY")
            .env_remove("GROK_AUTH_PROVIDER_COMMAND")
            .env_remove("ZUSE_GROK_AUTH_SOCKET")
            .env_remove("GROK_OIDC_ISSUER")
            .env_remove("GROK_OIDC_CLIENT_ID");
        for id in zuse_protocol::cloud_computers::ProviderId::ALL {
            command.env_remove(id.api_key_env());
        }
        for (key, value) in &self.values {
            command.env(key, value.as_str());
        }
    }
}
#[async_trait]
pub trait CredentialResolver: Send + Sync {
    async fn environment(&self) -> Result<ChildEnvironment, HarnessError>;
    /// Check authority state at a user-delivery boundary. Cloud resolvers must
    /// bypass cached grants here, without forcing an OAuth-token refresh.
    async fn before_turn(&self) -> Result<ChildEnvironment, HarnessError> {
        self.environment().await
    }
}

/// A retained child cannot reread its parent's environment. Revalidate the
/// selected authority before delivery, and require explicit runtime repair if
/// a static credential changed. External-auth commands keep the same launch
/// environment while fetching renewed access tokens through their callback.
pub(crate) struct LiveCredentials {
    resolver: std::sync::Arc<dyn CredentialResolver>,
    fingerprint: [u8; 32],
}
impl LiveCredentials {
    pub(crate) fn new(
        resolver: std::sync::Arc<dyn CredentialResolver>,
        environment: &ChildEnvironment,
    ) -> Self {
        Self {
            resolver,
            fingerprint: environment.fingerprint(),
        }
    }
    pub(crate) async fn validate(&self) -> Result<(), HarnessError> {
        let failure = || HarnessError::Protocol("agent-connection-requires-runtime-restart".into());
        let environment = tokio::time::timeout(
            std::time::Duration::from_secs(8),
            self.resolver.before_turn(),
        )
        .await
        .map_err(|_| failure())?
        .map_err(|_| failure())?;
        if environment.fingerprint() != self.fingerprint {
            return Err(failure());
        }
        Ok(())
    }
}
impl ChildEnvironment {
    fn fingerprint(&self) -> [u8; 32] {
        use sha2::{Digest, Sha256};
        let mut digest = Sha256::new();
        for (key, value) in &self.values {
            digest.update((key.len() as u64).to_be_bytes());
            digest.update(key.as_bytes());
            digest.update((value.len() as u64).to_be_bytes());
            digest.update(value.as_bytes());
        }
        digest.finalize().into()
    }
}
#[cfg(test)]
mod retained_tests {
    use super::*;
    struct Resolver(std::sync::Mutex<Option<&'static str>>);
    #[async_trait]
    impl CredentialResolver for Resolver {
        async fn environment(&self) -> Result<ChildEnvironment, HarnessError> {
            self.0
                .lock()
                .unwrap()
                .map(|value| {
                    ChildEnvironment::new("ANTHROPIC_API_KEY", Zeroizing::new(value.into()))
                })
                .ok_or_else(|| HarnessError::Protocol("private failure".into()))
        }
    }
    #[tokio::test]
    async fn retained_static_credentials_reject_rotation_and_revocation() {
        let resolver =
            std::sync::Arc::new(Resolver(std::sync::Mutex::new(Some("old-private-key"))));
        let live = LiveCredentials::new(resolver.clone(), &resolver.environment().await.unwrap());
        live.validate().await.unwrap();
        *resolver.0.lock().unwrap() = Some("new-private-key");
        let error = live.validate().await.unwrap_err().to_string();
        assert_eq!(
            error,
            "harness protocol error: agent-connection-requires-runtime-restart"
        );
        assert!(!error.contains("private"));
        *resolver.0.lock().unwrap() = None;
        assert!(live.validate().await.is_err());
    }
    #[test]
    fn apply_removes_every_cloud_provider_key() {
        let mut command = Command::new("probe");
        ChildEnvironment::empty().apply(&mut command);
        let removed: Vec<String> = command
            .as_std_mut()
            .get_envs()
            .filter(|(_, value)| value.is_none())
            .map(|(key, _)| key.to_string_lossy().into_owned())
            .collect();
        for id in zuse_protocol::cloud_computers::ProviderId::ALL {
            assert!(
                removed.iter().any(|key| key == id.api_key_env()),
                "{} reaches brokered processes",
                id.api_key_env()
            );
        }
    }
}

#[cfg(test)]
mod redaction_tests;
