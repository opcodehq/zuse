//! Existing account-authority grant contracts. Ciphertext is opaque to the API.
use serde::{Deserialize, Serialize};
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CloudAuthProvider {
    Claude,
    Codex,
    Cursor,
    Grok,
}
impl CloudAuthProvider {
    pub fn id(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Cursor => "cursor",
            Self::Grok => "grok",
        }
    }
    pub fn reconnecting(self) -> &'static str {
        match self {
            Self::Claude => "claude-auth-reconnecting",
            Self::Codex => "codex-auth-reconnecting",
            Self::Cursor => "cursor-auth-reconnecting",
            Self::Grok => "grok-auth-reconnecting",
        }
    }
    pub fn reconnect_required(self) -> &'static str {
        match self {
            Self::Claude => "claude-auth-reconnect-required",
            Self::Codex => "codex-auth-reconnect-required",
            Self::Cursor => "cursor-auth-reconnect-required",
            Self::Grok => "grok-auth-reconnect-required",
        }
    }
}
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GrantReason {
    Initial,
    Proactive,
    Unauthorized,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderGrantRequest {
    pub request_id: String,
    pub protocol_version: u32,
    pub runtime_generation: i64,
    pub credential_public_jwk: String,
    pub reason: GrantReason,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_provider_account_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_chatgpt_account_id: Option<String>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SealedProviderGrant {
    pub protocol_version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<CloudAuthProvider>,
    pub request_id: String,
    pub key_thumbprint: String,
    pub authority_incarnation_id: String,
    pub authority_epoch: i64,
    pub wrapped_key: String,
    pub iv: String,
    pub ciphertext: String,
    pub tag: String,
}
impl SealedProviderGrant {
    /// Field order is the original JSON.stringify order, including the optional Codex discriminator.
    pub fn additional_data(&self) -> Vec<u8> {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Aad<'a> {
            protocol_version: u32,
            #[serde(skip_serializing_if = "Option::is_none")]
            provider_id: Option<CloudAuthProvider>,
            request_id: &'a str,
            key_thumbprint: &'a str,
            authority_incarnation_id: &'a str,
            authority_epoch: i64,
        }
        serde_json::to_vec(&Aad {
            protocol_version: self.protocol_version,
            provider_id: self.provider_id,
            request_id: &self.request_id,
            key_thumbprint: &self.key_thumbprint,
            authority_incarnation_id: &self.authority_incarnation_id,
            authority_epoch: self.authority_epoch,
        })
        .expect("grant metadata serializes")
    }
}
