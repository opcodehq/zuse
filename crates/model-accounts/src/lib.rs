//! Model account registrations and credentials owned by the Rust engine.
//! Public metadata is deliberately separate from encrypted credential records.
mod oauth;
pub mod vault;
pub use oauth::ModelAccounts;
use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, thiserror::Error)]
pub enum Error {
    #[error("Model account storage is unavailable.")]
    Storage,
    #[error("Model account service is unavailable. Retry when connected.")]
    Unavailable,
    #[error("This model registration is unknown.")]
    Unknown,
    #[error("Model sign-in could not be verified.")]
    Verification,
    #[error("The model account changed while the request was running.")]
    Changed,
    #[error("Reconnect this model account before continuing.")]
    Reauthorization,
    #[error("This model account cannot renew yet. Retry after its refresh window opens.")]
    RefreshNotAvailable,
    #[error("Model sign-in was cancelled.")]
    Cancelled,
    #[error("Model sign-in expired. Start again.")]
    Expired,
    #[error("Model sign-in was denied.")]
    Denied,
    #[error("Use a nonempty name of at most 200 characters.")]
    InvalidName,
    #[error("A model sign-in is already active.")]
    Busy,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    Chatgpt,
    Supergrok,
}
impl Provider {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Chatgpt => "chatgpt",
            Self::Supergrok => "supergrok",
        }
    }
    pub fn issuer(self) -> &'static str {
        match self {
            Self::Chatgpt => "https://auth.openai.com",
            Self::Supergrok => "https://auth.x.ai",
        }
    }
    pub fn scope(self) -> &'static str {
        match self {
            Self::Chatgpt => {
                "openid profile email offline_access resource.invoke chatgpt.tokens.use.direct"
            }
            Self::Supergrok => "openid profile email offline_access grok-cli:access api:access",
        }
    }
    pub fn required_scope(self) -> &'static str {
        match self {
            Self::Chatgpt => "chatgpt.tokens.use.direct",
            Self::Supergrok => "api:access",
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ConnectionState {
    Connected,
    PermissionRequired,
    Disconnected,
    Pending,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Connection {
    pub provider: Provider,
    pub storage: String,
    pub id: String,
    pub client_id: String,
    pub name: String,
    pub email: Option<String>,
    pub created_at: u64,
    pub preferred: bool,
    pub authorized: bool,
    pub status: ConnectionState,
    pub plan_notice_seen: bool,
}
// Token-bearing types intentionally do not implement Debug.
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Registration {
    pub id: String,
    pub client_id: String,
    pub subject: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    pub name: String,
    pub created_at: u64,
    pub preferred: bool,
    #[serde(default)]
    pub plan_notice_seen: bool,
    #[serde(default)]
    pub authorization_generation: u64,
    pub scope: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id_token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub access_token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub earliest_refresh_at: Option<u64>,
}

impl Registration {
    pub fn authorized(&self, provider: Provider) -> bool {
        self.access_token.as_ref().is_some_and(|s| !s.is_empty())
            && self.refresh_token.as_ref().is_some_and(|s| !s.is_empty())
            && self
                .scope
                .split_whitespace()
                .any(|s| s == provider.required_scope())
    }
    pub fn metadata(&self, provider: Provider, storage: &str) -> Connection {
        let authorized = self.authorized(provider);
        Connection {
            provider,
            storage: storage.into(),
            id: self.id.clone(),
            client_id: self.client_id.clone(),
            name: self.name.clone(),
            email: self.email.clone(),
            created_at: self.created_at,
            preferred: self.preferred,
            authorized,
            status: if authorized {
                ConnectionState::Connected
            } else if self.id_token.is_some() || self.access_token.is_some() {
                ConnectionState::PermissionRequired
            } else {
                ConnectionState::Disconnected
            },
            plan_notice_seen: self.plan_notice_seen,
        }
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingRegistration {
    pub id: String,
    pub client_id: String,
    pub created_at: u64,
}
pub struct Credential {
    pub connection_id: String,
    pub provider: Provider,
    pub access_token: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum SignInEvent {
    Url {
        url: String,
    },
    Device {
        user_code: String,
        verification_url: String,
        expires_at: u64,
    },
    Connected {
        connection: Connection,
    },
    Error {
        message: String,
    },
}
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u64::MAX as u128) as u64
}
