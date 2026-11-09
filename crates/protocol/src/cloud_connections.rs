//! Public agent-connection metadata. Credential input must never derive Debug.
use super::cloud_computers::AgentId;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ConnectionMethod {
    ApiKey,
    SetupToken,
    DeviceCode,
}
impl ConnectionMethod {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ApiKey => "api-key",
            Self::SetupToken => "setup-token",
            Self::DeviceCode => "device-code",
        }
    }
    pub fn supported(self, agent: AgentId) -> bool {
        match self {
            Self::ApiKey => true,
            Self::SetupToken => agent == AgentId::Claude,
            Self::DeviceCode => matches!(agent, AgentId::Codex | AgentId::Grok),
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ConnectionState {
    Pending,
    Connected,
    ReconnectRequired,
    Revoked,
    Error,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentConnection {
    pub connection_id: String,
    pub agent: AgentId,
    pub method: ConnectionMethod,
    pub state: ConnectionState,
    pub account_label: Option<String>,
    pub expires_at: Option<i64>,
    pub version: i64,
    pub error_code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential_update: Option<LoginOperation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential_update_key: Option<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConfigureConnection {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connection_id: Option<String>,
    pub agent: AgentId,
    pub method: ConnectionMethod,
    pub secret: String,
    pub idempotency_key: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StartConnectionLogin {
    pub agent: AgentId,
    pub connection_id: Option<String>,
    pub idempotency_key: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LoginState {
    Queued,
    Authorizing,
    Connected,
    Expired,
    Cancelled,
    Error,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginOperation {
    pub operation_id: String,
    pub connection_id: String,
    pub agent: AgentId,
    pub state: LoginState,
    pub verification_url: Option<String>,
    pub verification_code: Option<String>,
    pub expires_at: i64,
    pub error_code: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn supported_auth_methods_match_each_real_harness() {
        assert!(ConnectionMethod::SetupToken.supported(AgentId::Claude));
        assert!(!ConnectionMethod::DeviceCode.supported(AgentId::Claude));
        for agent in [AgentId::Codex, AgentId::Grok] {
            assert!(ConnectionMethod::DeviceCode.supported(agent));
            assert!(!ConnectionMethod::SetupToken.supported(agent));
        }
        for agent in [AgentId::Claude, AgentId::Codex, AgentId::Grok] {
            assert!(ConnectionMethod::ApiKey.supported(agent));
        }
    }
    #[test]
    fn secret_input_refuses_unexpected_unowned_fields() {
        assert!(serde_json::from_value::<ConfigureConnection>(serde_json::json!({"agent":"codex","method":"api-key","secret":"value","ownerId":"other-account"})).is_err());
    }
}
