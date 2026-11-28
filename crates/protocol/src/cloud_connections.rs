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
    /// Remove terminal wrapping from Claude setup tokens before submission or storage.
    pub fn normalize_secret(self, agent: AgentId, secret: &str) -> String {
        if agent == AgentId::Claude && self == Self::SetupToken {
            secret.chars().filter(|c| !c.is_whitespace()).collect()
        } else {
            secret.trim().to_owned()
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
    fn wrapped_claude_setup_token_is_normalized() {
        assert_eq!(
            ConnectionMethod::SetupToken.normalize_secret(
                AgentId::Claude,
                " \tsk-ant-oat01-fake_part\r\n second-part\u{a0}third\n",
            ),
            "sk-ant-oat01-fake_partsecond-partthird",
        );
        assert_eq!(
            ConnectionMethod::ApiKey.normalize_secret(AgentId::Claude, " key with spaces "),
            "key with spaces",
        );
    }

    #[test]
    fn secret_input_refuses_unexpected_unowned_fields() {
        assert!(serde_json::from_value::<ConfigureConnection>(serde_json::json!({"agent":"codex","method":"api-key","secret":"value","ownerId":"other-account"})).is_err());
    }
}
