//! Open original authority envelopes only in the enrolled guest; never persist plaintext credentials.
use crate::{Authority, Identity, now_ms};
use aes_gcm::{
    Aes256Gcm, KeyInit, Nonce,
    aead::{Aead, Payload},
};
use anyhow::{Result, ensure};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD as B64};
use rsa::{Oaep, RsaPrivateKey};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::sync::Arc;
use tokio::sync::{Mutex, watch};
use zeroize::Zeroizing;
use zuse_protocol::{
    cloud::CloudRuntimeBootstrap,
    cloud_auth::{CloudAuthProvider, GrantReason, ProviderGrantRequest, SealedProviderGrant},
};

pub struct GrantRecipient {
    key: RsaPrivateKey,
    public_jwk: String,
    thumb: String,
    account: String,
    workspace: String,
    generation: i64,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Plaintext {
    zuse_account_id: String,
    workspace_id: String,
    runtime_generation: i64,
    key_thumbprint: String,
    authority_incarnation_id: String,
    authority_epoch: i64,
    provider_id: CloudAuthProvider,
    method: Option<String>,
    credential_kind: Option<String>,
    secret: Option<Secret>,
    issued_at: i64,
    expires_at: i64,
}
struct Secret(Zeroizing<String>);
impl<'de> Deserialize<'de> for Secret {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        Ok(Self(Zeroizing::new(String::deserialize(d)?)))
    }
}
pub struct Credential {
    pub method: String,
    pub kind: String,
    pub secret: Zeroizing<String>,
    pub expires_at: i64,
}
impl GrantRecipient {
    pub(crate) fn new(
        identity: &Identity,
        bootstrap: &CloudRuntimeBootstrap,
        key: RsaPrivateKey,
        public_jwk: String,
    ) -> Result<Self> {
        ensure!(
            identity
                .bootstrap()
                .is_some_and(|known| known.workspace_id == bootstrap.workspace_id
                    && known.zuse_account_id == bootstrap.zuse_account_id
                    && known.runtime_generation == bootstrap.runtime_generation),
            "runtime_grant_identity_mismatch"
        );
        let jwk: serde_json::Value = serde_json::from_str(&public_jwk)?;
        let thumb = B64.encode(Sha256::digest(serde_json::to_vec(
            &serde_json::json!({"e":jwk["e"],"kty":"RSA","n":jwk["n"]}),
        )?));
        Ok(Self {
            key,
            public_jwk,
            thumb,
            account: bootstrap.zuse_account_id.clone(),
            workspace: bootstrap.workspace_id.clone(),
            generation: bootstrap.runtime_generation,
        })
    }
    pub fn open(
        &self,
        provider: CloudAuthProvider,
        request: &str,
        sealed: &SealedProviderGrant,
        now: i64,
    ) -> Result<Credential> {
        ensure!(
            sealed.protocol_version == 1
                && sealed.provider_id == Some(provider)
                && sealed.request_id == request
                && sealed.key_thumbprint == self.thumb
                && sealed.authority_epoch > 0
                && !sealed.authority_incarnation_id.is_empty()
                && sealed.ciphertext.len() <= 32768,
            "provider_grant_binding_invalid"
        );
        let wrapped = B64.decode(&sealed.wrapped_key)?;
        let key = Zeroizing::new(
            self.key
                .decrypt(Oaep::new::<Sha256>(), &wrapped)
                .map_err(|_| anyhow::anyhow!("provider_grant_decryption_failed"))?,
        );
        let iv = B64.decode(&sealed.iv)?;
        let tag = B64.decode(&sealed.tag)?;
        ensure!(
            key.len() == 32 && iv.len() == 12 && tag.len() == 16,
            "provider_grant_decryption_failed"
        );
        let mut ciphertext = B64.decode(&sealed.ciphertext)?;
        ciphertext.extend_from_slice(&tag);
        let cipher = Aes256Gcm::new_from_slice(&key)
            .map_err(|_| anyhow::anyhow!("provider_grant_decryption_failed"))?;
        let bytes = Zeroizing::new(
            cipher
                .decrypt(
                    Nonce::from_slice(&iv),
                    Payload {
                        msg: &ciphertext,
                        aad: &sealed.additional_data(),
                    },
                )
                .map_err(|_| anyhow::anyhow!("provider_grant_decryption_failed"))?,
        );
        let value: Plaintext = serde_json::from_slice(&bytes)
            .map_err(|_| anyhow::anyhow!("provider_grant_payload_invalid"))?;
        ensure!(
            value.zuse_account_id == self.account
                && value.workspace_id == self.workspace
                && value.runtime_generation == self.generation
                && value.provider_id == provider
                && value.key_thumbprint == sealed.key_thumbprint
                && value.authority_incarnation_id == sealed.authority_incarnation_id
                && value.authority_epoch == sealed.authority_epoch
                && value.issued_at <= now + 60_000
                && value.expires_at > now + 60_000,
            "provider_grant_payload_binding_invalid"
        );
        let method = value
            .method
            .filter(|method| matches!(method.as_str(), "subscription" | "api-key" | "custom"))
            .ok_or_else(|| anyhow::anyhow!("provider_grant_method_unsupported"))?;
        let kind = value
            .credential_kind
            .filter(|kind| matches!(kind.as_str(), "api-key" | "oauth-token"))
            .ok_or_else(|| anyhow::anyhow!("provider_grant_kind_unsupported"))?;
        let secret = value
            .secret
            .ok_or_else(|| anyhow::anyhow!("provider_grant_secret_missing"))?
            .0;
        ensure!(!secret.is_empty(), "provider_grant_secret_missing");
        Ok(Credential {
            method,
            kind,
            secret,
            expires_at: value.expires_at,
        })
    }
}
struct State {
    cached: Option<Credential>,
    request: Option<String>,
}
pub struct ProviderBroker {
    recipient: GrantRecipient,
    provider: CloudAuthProvider,
    authority: Authority,
    bootstrap: watch::Receiver<CloudRuntimeBootstrap>,
    state: Mutex<State>,
}
impl ProviderBroker {
    pub fn new(
        recipient: GrantRecipient,
        provider: CloudAuthProvider,
        authority: Authority,
        bootstrap: watch::Receiver<CloudRuntimeBootstrap>,
    ) -> Arc<Self> {
        Arc::new(Self {
            recipient,
            provider,
            authority,
            bootstrap,
            state: Mutex::new(State {
                cached: None,
                request: None,
            }),
        })
    }
    pub async fn credential(&self) -> Result<Credential> {
        let mut state = self.state.lock().await;
        if let Some(cached) = &state.cached
            && cached.expires_at > now_ms() + 300_000
        {
            return Ok(Credential {
                method: cached.method.clone(),
                kind: cached.kind.clone(),
                secret: Zeroizing::new(cached.secret.to_string()),
                expires_at: cached.expires_at,
            });
        }
        let id = state
            .request
            .get_or_insert_with(|| uuid::Uuid::new_v4().to_string())
            .clone();
        let boot = self.bootstrap.borrow().clone();
        ensure!(
            boot.runtime_generation == self.recipient.generation
                && boot.workspace_id == self.recipient.workspace,
            "runtime_grant_identity_mismatch"
        );
        let request = ProviderGrantRequest {
            request_id: id.clone(),
            protocol_version: 1,
            runtime_generation: boot.runtime_generation,
            credential_public_jwk: self.recipient.public_jwk.clone(),
            reason: if state.cached.is_some() {
                GrantReason::Proactive
            } else {
                GrantReason::Initial
            },
            previous_provider_account_id: None,
            previous_chatgpt_account_id: None,
        };
        let response = self
            .authority
            .post(
                &boot.workspace_id,
                &["runtime", "providers", self.provider.id(), "grant"],
                &boot.runtime_credential,
                &serde_json::to_value(request)?,
            )
            .await?;
        let sealed: SealedProviderGrant = serde_json::from_value(response)
            .map_err(|_| anyhow::anyhow!("provider_grant_response_invalid"))?;
        let value = self.recipient.open(self.provider, &id, &sealed, now_ms())?;
        let copy = Credential {
            method: value.method.clone(),
            kind: value.kind.clone(),
            secret: Zeroizing::new(value.secret.to_string()),
            expires_at: value.expires_at,
        };
        state.cached = Some(value);
        state.request = None;
        Ok(copy)
    }
}
#[cfg(feature = "engine")]
#[async_trait::async_trait]
impl zuse_providers::credentials::CredentialResolver for ProviderBroker {
    async fn environment(
        &self,
    ) -> std::result::Result<
        zuse_providers::credentials::ChildEnvironment,
        zuse_providers::HarnessError,
    > {
        let credential = self.credential().await.map_err(|_| {
            zuse_providers::HarnessError::Protocol(self.provider.reconnecting().into())
        })?;
        if self.provider != CloudAuthProvider::Claude || credential.method == "custom" {
            return Err(zuse_providers::HarnessError::Protocol(
                "provider_grant_adapter_required".into(),
            ));
        }
        let key = if credential.kind == "oauth-token" {
            "CLAUDE_CODE_OAUTH_TOKEN"
        } else {
            "ANTHROPIC_API_KEY"
        };
        Ok(zuse_providers::credentials::ChildEnvironment::new(
            key,
            credential.secret,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rsa::pkcs8::DecodePrivateKey;
    #[test]
    fn original_node_grants_decrypt_and_reject_changed_scope_expiry_and_authenticated_metadata() {
        let keys: serde_json::Value =
            serde_json::from_str(include_str!("../../cloud/tests/fixtures/cloud-crypto.json"))
                .unwrap();
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../tests/fixtures/provider-grant.json")).unwrap();
        let recipient = GrantRecipient {
            key: RsaPrivateKey::from_pkcs8_der(
                &B64.decode(keys["runtimePrivatePkcs8"].as_str().unwrap())
                    .unwrap(),
            )
            .unwrap(),
            public_jwk: keys["runtimePublicJwk"].to_string(),
            thumb: keys["runtimeThumbprint"].as_str().unwrap().into(),
            account: "user-original".into(),
            workspace: "workspace-original".into(),
            generation: 2,
        };
        let request = fixture["requestId"].as_str().unwrap();
        let now = fixture["now"].as_i64().unwrap();
        let good: SealedProviderGrant = serde_json::from_value(fixture["good"].clone()).unwrap();
        let credential = recipient
            .open(CloudAuthProvider::Claude, request, &good, now)
            .unwrap();
        assert_eq!(credential.secret.as_str(), "synthetic-key-fixture");
        for name in ["wrongAccount", "wrongGeneration", "shortLived"] {
            let sealed = serde_json::from_value(fixture[name].clone()).unwrap();
            assert!(
                recipient
                    .open(CloudAuthProvider::Claude, request, &sealed, now)
                    .is_err()
            );
        }
        let mut changed = good.clone();
        changed.authority_epoch += 1;
        assert!(
            recipient
                .open(CloudAuthProvider::Claude, request, &changed, now)
                .is_err()
        );
        let mut changed = good.clone();
        changed.provider_id = Some(CloudAuthProvider::Cursor);
        assert!(
            recipient
                .open(CloudAuthProvider::Claude, request, &changed, now)
                .is_err()
        );
        let mut changed = good;
        let mut ciphertext = B64.decode(&changed.ciphertext).unwrap();
        ciphertext[0] ^= 1;
        changed.ciphertext = B64.encode(ciphertext);
        assert!(
            recipient
                .open(CloudAuthProvider::Claude, request, &changed, now)
                .is_err()
        );
    }
}
