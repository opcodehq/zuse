//! Ephemeral credentials supplied by the host, outside serialized run requests and history.
use crate::{HarnessError, process::Command};
use async_trait::async_trait;
use zeroize::Zeroizing;
pub struct ChildEnvironment {
    key: &'static str,
    value: Zeroizing<String>,
}
impl ChildEnvironment {
    pub fn new(key: &'static str, value: Zeroizing<String>) -> Self {
        Self { key, value }
    }
    pub fn apply(&self, command: &mut Command) {
        // A brokered Claude process must never fall back to a different inherited account.
        command
            .env_remove("ANTHROPIC_API_KEY")
            .env_remove("CLAUDE_CODE_OAUTH_TOKEN")
            .env_remove("ANTHROPIC_AUTH_TOKEN");
        command.env(self.key, self.value.as_str());
    }
}
#[async_trait]
pub trait CredentialResolver: Send + Sync {
    async fn environment(&self) -> Result<ChildEnvironment, HarnessError>;
}
