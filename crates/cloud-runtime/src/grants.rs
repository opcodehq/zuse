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
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
use tokio::sync::{Mutex, watch};
use zeroize::Zeroizing;
use zuse_protocol::{
    cloud::CloudRuntimeBootstrap,
    cloud_auth::{CloudAuthProvider, GrantReason, ProviderGrantRequest, SealedProviderGrant},
};
#[cfg(feature = "engine")]
mod brokers;
#[cfg(feature = "engine")]
pub use brokers::ProviderBrokers;

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
    provider_id: Option<CloudAuthProvider>,
    provider_account_id: Option<String>,
    chatgpt_account_id: Option<String>,
    chatgpt_plan_type: Option<String>,
    access_token: Option<Secret>,
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
    pub account_id: Option<String>,
    pub plan_type: Option<String>,
}
impl Credential {
    fn copy(&self) -> Self {
        Self {
            method: self.method.clone(),
            kind: self.kind.clone(),
            secret: Zeroizing::new(self.secret.to_string()),
            expires_at: self.expires_at,
            account_id: self.account_id.clone(),
            plan_type: self.plan_type.clone(),
        }
    }
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
                && (sealed.provider_id == Some(provider)
                    || (provider == CloudAuthProvider::Codex && sealed.provider_id.is_none()))
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
                && (value.provider_id == Some(provider)
                    || (provider == CloudAuthProvider::Codex && value.provider_id.is_none()))
                && (sealed.provider_id.is_some()
                    || value.provider_id.is_none()
                    || value.provider_id == Some(CloudAuthProvider::Codex))
                && value.key_thumbprint == sealed.key_thumbprint
                && value.authority_incarnation_id == sealed.authority_incarnation_id
                && value.authority_epoch == sealed.authority_epoch
                && value.issued_at <= now + 60_000
                && value.expires_at > now + 60_000,
            "provider_grant_payload_binding_invalid"
        );
        // The original device-auth Codex authority uses an access-token-only envelope.
        // Its discriminator is absent in both authenticated metadata and payload.
        let legacy_codex = provider == CloudAuthProvider::Codex && sealed.provider_id.is_none();
        let (method, kind, secret) = if legacy_codex {
            ensure!(
                value
                    .chatgpt_account_id
                    .as_ref()
                    .is_some_and(|id| !id.is_empty() && id.len() <= 256),
                "provider_grant_account_missing"
            );
            (
                "subscription".to_owned(),
                "oauth-token".to_owned(),
                value.access_token,
            )
        } else {
            let method = value
                .method
                .filter(|m| matches!(m.as_str(), "subscription" | "api-key" | "custom"))
                .ok_or_else(|| anyhow::anyhow!("provider_grant_method_unsupported"))?;
            let kind = value
                .credential_kind
                .filter(|k| matches!(k.as_str(), "api-key" | "oauth-token"))
                .ok_or_else(|| anyhow::anyhow!("provider_grant_kind_unsupported"))?;
            (method, kind, value.secret)
        };
        let secret = secret
            .ok_or_else(|| anyhow::anyhow!("provider_grant_secret_missing"))?
            .0;
        ensure!(!secret.is_empty(), "provider_grant_secret_missing");
        Ok(Credential {
            method,
            kind,
            secret,
            expires_at: value.expires_at,
            account_id: value.provider_account_id.or(value.chatgpt_account_id),
            plan_type: value.chatgpt_plan_type,
        })
    }
}
struct State {
    cached: Option<Credential>,
    rejected: bool,
    request: Option<(String, bool, Option<String>)>,
}
pub struct ProviderBroker {
    #[cfg(feature = "engine")]
    native_memory_sessions: Arc<zuse_providers::codex::memory::ManagedMemorySessions>,
    #[cfg(feature = "engine")]
    secret_filter: std::sync::OnceLock<Arc<zuse_providers::credentials::redaction::Secrets>>,
    recipient: GrantRecipient,
    provider: CloudAuthProvider,
    authority: Authority,
    bootstrap: watch::Receiver<CloudRuntimeBootstrap>,
    state: Mutex<State>,
    revision: AtomicU64,
}
impl ProviderBroker {
    pub fn new(
        recipient: GrantRecipient,
        provider: CloudAuthProvider,
        authority: Authority,
        bootstrap: watch::Receiver<CloudRuntimeBootstrap>,
    ) -> Arc<Self> {
        Arc::new(Self {
            #[cfg(feature = "engine")]
            native_memory_sessions: zuse_providers::codex::memory::ManagedMemorySessions::new(),
            #[cfg(feature = "engine")]
            secret_filter: std::sync::OnceLock::new(),
            revision: AtomicU64::new(0),
            recipient,
            provider,
            authority,
            bootstrap,
            state: Mutex::new(State {
                cached: None,
                rejected: false,
                request: None,
            }),
        })
    }
    #[cfg(feature = "engine")]
    pub fn native_memory_sessions(
        &self,
    ) -> Arc<zuse_providers::codex::memory::ManagedMemorySessions> {
        self.native_memory_sessions.clone()
    }
    #[cfg(feature = "engine")]
    pub fn set_secret_filter(
        &self,
        filter: Arc<zuse_providers::credentials::redaction::Secrets>,
    ) -> Result<()> {
        if let Ok(state) = self.state.try_lock() {
            if let Some(credential) = &state.cached {
                filter.record(&credential.secret)?;
            }
        }
        self.secret_filter
            .set(filter)
            .map_err(|_| anyhow::anyhow!("runtime_secret_filter_already_configured"))
    }
    pub async fn credential(&self) -> Result<Credential> {
        self.resolve_credential(false, None).await
    }
    pub(crate) async fn resolve_credential(
        &self,
        unauthorized: bool,
        previous: Option<&str>,
    ) -> Result<Credential> {
        let observed = self.revision.load(Ordering::Acquire);
        let mut state = self.state.lock().await;
        let boot = self.bootstrap.borrow().clone();
        ensure!(
            boot.runtime_generation == self.recipient.generation
                && boot.workspace_id == self.recipient.workspace
                && boot.zuse_account_id == self.recipient.account,
            "runtime_grant_identity_mismatch"
        );
        if let Some(previous) = previous {
            ensure!(
                state
                    .cached
                    .as_ref()
                    .is_none_or(|c| c.account_id.as_deref() == Some(previous)),
                "provider_grant_account_changed"
            );
        }
        if let Some(cached) = &state.cached
            && ((!unauthorized && !state.rejected && cached.expires_at > now_ms() + 300_000)
                || (self.revision.load(Ordering::Acquire) != observed
                    && cached.expires_at > now_ms() + 60_000))
        {
            return Ok(cached.copy());
        }
        let unauthorized = unauthorized || state.rejected;
        if unauthorized {
            state.rejected = true;
        }
        let previous = previous
            .map(str::to_owned)
            .or_else(|| state.cached.as_ref().and_then(|c| c.account_id.clone()));
        if state
            .request
            .as_ref()
            .is_none_or(|(_, force, account)| *force != unauthorized || account != &previous)
        {
            state.request = Some((
                uuid::Uuid::new_v4().to_string(),
                unauthorized,
                previous.clone(),
            ));
        }
        let id = state
            .request
            .as_ref()
            .expect("grant request initialized")
            .0
            .clone();
        let request = ProviderGrantRequest {
            request_id: id.clone(),
            protocol_version: 1,
            runtime_generation: boot.runtime_generation,
            credential_public_jwk: self.recipient.public_jwk.clone(),
            reason: if unauthorized {
                GrantReason::Unauthorized
            } else if state.cached.is_some() {
                GrantReason::Proactive
            } else {
                GrantReason::Initial
            },
            previous_provider_account_id: (self.provider != CloudAuthProvider::Codex)
                .then(|| previous.clone())
                .flatten(),
            previous_chatgpt_account_id: (self.provider == CloudAuthProvider::Codex)
                .then(|| previous.clone())
                .flatten(),
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
        let latest = self.bootstrap.borrow();
        ensure!(
            latest.runtime_generation == self.recipient.generation
                && latest.workspace_id == self.recipient.workspace
                && latest.zuse_account_id == self.recipient.account,
            "runtime_grant_identity_mismatch"
        );
        ensure!(
            previous
                .as_ref()
                .is_none_or(|previous| value.account_id.as_ref() == Some(previous)),
            "provider_grant_account_changed"
        );
        #[cfg(feature = "engine")]
        if let Some(filter) = self.secret_filter.get() {
            filter.record(&value.secret)?;
        }
        let copy = value.copy();
        state.rejected = false;
        state.cached = Some(value);
        state.request = None;
        self.revision.fetch_add(1, Ordering::Release);
        Ok(copy)
    }
}
#[cfg(feature = "engine")]
#[async_trait::async_trait]
impl zuse_providers::credentials::CredentialResolver for ProviderBroker {
    async fn before_turn(
        &self,
    ) -> std::result::Result<
        zuse_providers::credentials::ChildEnvironment,
        zuse_providers::HarnessError,
    > {
        self.resume_credential().await.map_err(|_| {
            zuse_providers::HarnessError::Protocol(self.provider.reconnecting().into())
        })?;
        zuse_providers::credentials::CredentialResolver::environment(self).await
    }
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

#[cfg(feature = "engine")]
#[async_trait::async_trait]
impl zuse_providers::codex::auth::CredentialResolver for ProviderBroker {
    async fn before_turn(
        &self,
    ) -> std::result::Result<zuse_providers::codex::auth::Credential, zuse_providers::HarnessError>
    {
        self.resume_credential().await.map_err(|_| {
            zuse_providers::HarnessError::Protocol("codex-auth-reconnecting".into())
        })?;
        zuse_providers::codex::auth::CredentialResolver::resolve(self, false, None).await
    }
    async fn resolve(
        &self,
        unauthorized: bool,
        previous_account: Option<&str>,
    ) -> std::result::Result<zuse_providers::codex::auth::Credential, zuse_providers::HarnessError>
    {
        use zuse_providers::codex::auth::Credential as Codex;
        let error = || zuse_providers::HarnessError::Protocol("codex-auth-reconnecting".into());
        if self.provider != CloudAuthProvider::Codex {
            return Err(error());
        }
        let credential = self
            .resolve_credential(unauthorized, previous_account)
            .await
            .map_err(|_| error())?;
        match (credential.method.as_str(), credential.kind.as_str()) {
            ("api-key", "api-key") => Ok(Codex::ApiKey(credential.secret)),
            ("subscription", "oauth-token") => Ok(Codex::ChatGpt {
                access_token: credential.secret,
                account_id: credential
                    .account_id
                    .filter(|s| !s.is_empty())
                    .ok_or_else(error)?,
                plan_type: credential.plan_type,
            }),
            _ => Err(error()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rsa::pkcs8::DecodePrivateKey;
    fn recipient() -> GrantRecipient {
        let keys: serde_json::Value =
            serde_json::from_str(include_str!("../../cloud/tests/fixtures/cloud-crypto.json"))
                .unwrap();
        GrantRecipient {
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
        }
    }
    fn seal(
        recipient: &GrantRecipient,
        request: &str,
        now: i64,
        account: &str,
    ) -> SealedProviderGrant {
        let key = [7u8; 32];
        let iv = [9u8; 12]; // Synthetic fixtures only, never used outside these tests.
        let mut sealed = SealedProviderGrant {
            protocol_version: 1,
            provider_id: None,
            request_id: request.into(),
            key_thumbprint: recipient.thumb.clone(),
            authority_incarnation_id: "authority-test".into(),
            authority_epoch: 1,
            wrapped_key: B64.encode(
                rsa::RsaPublicKey::from(&recipient.key)
                    .encrypt(&mut rand::thread_rng(), Oaep::new::<Sha256>(), &key)
                    .unwrap(),
            ),
            iv: B64.encode(iv),
            ciphertext: String::new(),
            tag: String::new(),
        };
        let plaintext = serde_json::json!({"zuseAccountId":"user-original","workspaceId":"workspace-original",
            "runtimeGeneration":2,"keyThumbprint":recipient.thumb,"authorityIncarnationId":"authority-test","authorityEpoch":1,
            "providerId":"codex","issuedAt":now,"expiresAt":now+3_600_000,"accessToken":"synthetic-codex-access",
            "chatgptAccountId":account,"chatgptPlanType":"test-plan"});
        let encrypted = Aes256Gcm::new_from_slice(&key)
            .unwrap()
            .encrypt(
                Nonce::from_slice(&iv),
                Payload {
                    msg: &serde_json::to_vec(&plaintext).unwrap(),
                    aad: &sealed.additional_data(),
                },
            )
            .unwrap();
        sealed.ciphertext = B64.encode(&encrypted[..encrypted.len() - 16]);
        sealed.tag = B64.encode(&encrypted[encrypted.len() - 16..]);
        sealed
    }
    #[test]
    fn device_auth_codex_grant_has_no_refresh_token_and_rejects_cross_provider_delivery() {
        let recipient = recipient();
        let sealed = seal(&recipient, "request", 1000, "account-a");
        let credential = recipient
            .open(CloudAuthProvider::Codex, "request", &sealed, 1000)
            .unwrap();
        assert_eq!(credential.secret.as_str(), "synthetic-codex-access");
        assert_eq!(credential.account_id.as_deref(), Some("account-a"));
        assert!(
            recipient
                .open(CloudAuthProvider::Claude, "request", &sealed, 1000)
                .is_err()
        );
        assert!(
            recipient
                .open(CloudAuthProvider::Codex, "other-request", &sealed, 1000)
                .is_err()
        );
        assert!(
            recipient
                .open(CloudAuthProvider::Codex, "request", &sealed, 3_550_000)
                .is_err()
        );
    }
    #[tokio::test]
    async fn broker_coalesces_grants_forces_unauthorized_refresh_and_fences_cached_accounts() {
        use axum::{Json, Router, routing::post};
        let calls = Arc::new(AtomicU64::new(0));
        let count = calls.clone();
        let app = Router::new().route(
            "/v1/cloud/workspaces/workspace-original/runtime/providers/codex/grant",
            post(move |Json(request): Json<ProviderGrantRequest>| {
                let count = count.clone();
                async move {
                    let number = count.fetch_add(1, Ordering::SeqCst) + 1;
                    if number >= 3 {
                        return Err(axum::http::StatusCode::SERVICE_UNAVAILABLE);
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(30)).await;
                    Ok(Json(seal(
                        &recipient(),
                        &request.request_id,
                        now_ms(),
                        "account-a",
                    )))
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let boot: CloudRuntimeBootstrap = serde_json::from_value(serde_json::json!({"workspaceId":"workspace-original","zuseAccountId":"user-original","chatId":"chat","initialSessionId":"session","runtimeCredential":"synthetic-runtime","runtimeGeneration":2,"gatewayEpoch":1,"runtimeCredentialExpiresAt":9999999999999i64,"gatewayUrl":"wss://test.invalid","gatewayProtocol":"test","sealedTranscriptKey":"synthetic"})).unwrap();
        let (updates, receiver) = watch::channel(boot.clone());
        let broker = ProviderBroker::new(
            recipient(),
            CloudAuthProvider::Codex,
            Authority::new(&origin).unwrap(),
            receiver,
        );
        #[cfg(feature = "engine")]
        let secret_filter = Arc::new(zuse_providers::credentials::redaction::Secrets::default());
        #[cfg(feature = "engine")]
        broker.set_secret_filter(secret_filter.clone()).unwrap();
        let (a, b) = tokio::join!(broker.credential(), broker.credential());
        a.unwrap();
        b.unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        #[cfg(feature = "engine")]
        assert_eq!(secret_filter.mask("synthetic-codex-access"), "[redacted]");
        let (a, b) = tokio::join!(
            broker.resolve_credential(true, Some("account-a")),
            broker.resolve_credential(true, Some("account-a"))
        );
        a.unwrap();
        b.unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert!(
            broker
                .resolve_credential(true, Some("account-b"))
                .await
                .is_err()
        );
        assert!(
            broker
                .resolve_credential(true, Some("account-a"))
                .await
                .is_err()
        );
        // A rejected, unexpired access token must not be reused after an authority failure.
        assert!(broker.credential().await.is_err());
        assert_eq!(calls.load(Ordering::SeqCst), 4);
        let mut changed = boot;
        changed.runtime_generation += 1;
        updates.send(changed).unwrap();
        assert!(broker.credential().await.is_err());
        assert_eq!(calls.load(Ordering::SeqCst), 4);
        server.abort();
    }
    #[tokio::test]
    async fn new_turn_after_uncertain_refresh_checks_authority_without_fabricating_unauthorized() {
        use axum::{Json, Router, routing::post};
        let reasons = Arc::new(Mutex::new(Vec::new()));
        let seen = reasons.clone();
        let app = Router::new().route(
            "/v1/cloud/workspaces/workspace-original/runtime/providers/codex/grant",
            post(move |Json(request): Json<ProviderGrantRequest>| {
                let seen = seen.clone();
                async move {
                    let mut seen = seen.lock().await;
                    seen.push(serde_json::to_value(&request.reason).unwrap());
                    if seen.len() == 2 {
                        return Err(axum::http::StatusCode::SERVICE_UNAVAILABLE);
                    }
                    Ok(Json(seal(
                        &recipient(),
                        &request.request_id,
                        now_ms(),
                        "account-a",
                    )))
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let boot:CloudRuntimeBootstrap=serde_json::from_value(serde_json::json!({"workspaceId":"workspace-original","zuseAccountId":"user-original","chatId":"chat","initialSessionId":"session","runtimeCredential":"synthetic-runtime","runtimeGeneration":2,"gatewayEpoch":1,"runtimeCredentialExpiresAt":9999999999999i64,"gatewayUrl":"wss://test.invalid","gatewayProtocol":"test","sealedTranscriptKey":"synthetic"})).unwrap();
        let (_updates, receiver) = watch::channel(boot);
        let broker = ProviderBroker::new(
            recipient(),
            CloudAuthProvider::Codex,
            Authority::new(&origin).unwrap(),
            receiver,
        );
        broker.credential().await.unwrap();
        assert!(
            broker
                .resolve_credential(true, Some("account-a"))
                .await
                .is_err()
        );
        broker.resume_credential().await.unwrap();
        assert_eq!(
            *reasons.lock().await,
            vec![
                serde_json::json!("initial"),
                serde_json::json!("unauthorized"),
                serde_json::json!("initial")
            ]
        );
        server.abort();
    }
    #[cfg(all(feature = "engine", unix))]
    #[tokio::test]
    async fn registering_three_managed_agents_fetches_only_the_selected_codex_grant() {
        use axum::{Json, Router, routing::post};
        let calls = Arc::new(AtomicU64::new(0));
        let other_calls = Arc::new(AtomicU64::new(0));
        let counted = calls.clone();
        let unexpected = other_calls.clone();
        let app = Router::new()
            .route(
                "/v1/cloud/workspaces/workspace-original/runtime/providers/codex/grant",
                post(move |Json(request): Json<ProviderGrantRequest>| {
                    let counted = counted.clone();
                    async move {
                        counted.fetch_add(1, Ordering::SeqCst);
                        Json(seal(
                            &recipient(),
                            &request.request_id,
                            now_ms(),
                            "account-a",
                        ))
                    }
                }),
            )
            .fallback(move || {
                let unexpected = unexpected.clone();
                async move {
                    unexpected.fetch_add(1, Ordering::SeqCst);
                    axum::http::StatusCode::SERVICE_UNAVAILABLE
                }
            });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let authority =
            Authority::new(&format!("http://{}", listener.local_addr().unwrap())).unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let boot:CloudRuntimeBootstrap=serde_json::from_value(serde_json::json!({"workspaceId":"workspace-original","zuseAccountId":"user-original","chatId":"chat","initialSessionId":"session","runtimeCredential":"synthetic-runtime","runtimeGeneration":2,"gatewayEpoch":1,"runtimeCredentialExpiresAt":9999999999999i64,"gatewayUrl":"wss://test.invalid","gatewayProtocol":"test","sealedTranscriptKey":"synthetic"})).unwrap();
        let (_updates, receiver) = watch::channel(boot);
        let codex = ProviderBroker::new(
            recipient(),
            CloudAuthProvider::Codex,
            authority.clone(),
            receiver.clone(),
        );
        let claude = ProviderBroker::new(
            recipient(),
            CloudAuthProvider::Claude,
            authority.clone(),
            receiver.clone(),
        );
        let grok = ProviderBroker::new(recipient(), CloudAuthProvider::Grok, authority, receiver);
        let root = tempfile::tempdir().unwrap();
        let profile = root.path().join("not-created-for-unselected-agent");
        let lazy = crate::grok_auth::LazyGrokAuth::new(
            grok.clone(),
            root.path().join("missing-unselected-helper"),
            profile.clone(),
        );
        let registry = zuse_engine::HarnessRegistry::new();
        registry.register(Arc::new(
            zuse_providers::ClaudeHarness::new().with_credentials(claude.clone()),
        ));
        registry.register(Arc::new(
            zuse_providers::CodexHarness::new().with_credentials(codex.clone()),
        ));
        registry.register(Arc::new(
            zuse_providers::AcpHarness::grok().with_credentials(lazy),
        ));
        assert_eq!(registry.descriptors().len(), 3);
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert_eq!(other_calls.load(Ordering::SeqCst), 0);
        assert!(!profile.exists());
        zuse_providers::codex::auth::CredentialResolver::resolve(codex.as_ref(), false, None)
            .await
            .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(other_calls.load(Ordering::SeqCst), 0);
        assert!(claude.state.lock().await.cached.is_none());
        assert!(grok.state.lock().await.cached.is_none());
        assert!(!profile.exists());
        server.abort();
    }

    /// Component benchmark: real HTTP and envelope crypto, synthetic account credentials.
    /// Run explicitly; wall-clock distributions do not belong in correctness CI assertions.
    #[tokio::test]
    #[ignore = "opt-in local credential timing benchmark"]
    async fn credential_delivery_timings() {
        use axum::{Json, Router, routing::post};
        use std::time::Instant;
        let count = Arc::new(AtomicU64::new(0));
        let calls = count.clone();
        let key = Arc::new(recipient());
        let app = Router::new().route(
            "/v1/cloud/workspaces/workspace-original/runtime/providers/codex/grant",
            post(move |Json(request): Json<ProviderGrantRequest>| {
                let key = key.clone();
                let calls = calls.clone();
                async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    Json(seal(&key, &request.request_id, now_ms(), "account-a"))
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let boot: CloudRuntimeBootstrap = serde_json::from_value(serde_json::json!({"workspaceId":"workspace-original","zuseAccountId":"user-original","chatId":"chat","initialSessionId":"session","runtimeCredential":"synthetic-runtime","runtimeGeneration":2,"gatewayEpoch":1,"runtimeCredentialExpiresAt":9999999999999i64,"gatewayUrl":"wss://test.invalid","gatewayProtocol":"test","sealedTranscriptKey":"synthetic"})).unwrap();
        let (_updates, receiver) = watch::channel(boot);
        let authority = Authority::new(&origin).unwrap();
        let mut cold = Vec::new();
        let mut renew = Vec::new();
        for _ in 0..30 {
            let broker = ProviderBroker::new(
                recipient(),
                CloudAuthProvider::Codex,
                authority.clone(),
                receiver.clone(),
            );
            let start = Instant::now();
            broker.credential().await.unwrap();
            cold.push(start.elapsed().as_secs_f64() * 1000.0);
            let start = Instant::now();
            broker
                .resolve_credential(true, Some("account-a"))
                .await
                .unwrap();
            renew.push(start.elapsed().as_secs_f64() * 1000.0);
        }
        let broker =
            ProviderBroker::new(recipient(), CloudAuthProvider::Codex, authority, receiver);
        broker.credential().await.unwrap();
        let before = count.load(Ordering::SeqCst);
        let mut cached = Vec::new();
        for _ in 0..1000 {
            let start = Instant::now();
            broker.credential().await.unwrap();
            cached.push(start.elapsed().as_secs_f64() * 1000.0);
        }
        assert_eq!(
            before,
            count.load(Ordering::SeqCst),
            "cached credentials must not make HTTP requests"
        );
        fn summary(mut values: Vec<f64>) -> serde_json::Value {
            values.sort_by(f64::total_cmp);
            serde_json::json!({"n":values.len(), "minMs":values[0], "medianMs":values[values.len()/2],
                "p95Ms":values[(values.len()*95).div_ceil(100)-1], "maxMs":values[values.len()-1]})
        }
        println!(
            "CREDENTIAL_TIMINGS={}",
            serde_json::json!({
                "scope":"localhost HTTP + production Rust sealed grant crypto; synthetic credentials; no real OAuth/model request",
                "initialGrant":summary(cold), "forcedRenewal":summary(renew), "cachedGrant":summary(cached),
                "cachedHttpRequests":count.load(Ordering::SeqCst)-before,
            })
        );
        server.abort();
    }
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

impl ProviderBroker {
    /// A resume checks the selected authority connection even when a cached grant
    /// has time remaining. It does not mark a valid provider token unauthorized.
    pub async fn resume_credential(&self) -> Result<()> {
        let previous = {
            let mut state = self.state.lock().await;
            let previous = state.cached.as_ref().and_then(|c| c.account_id.clone());
            state.cached = None;
            state.request = None;
            // This is an explicit new-turn authority check, not a new vendor401.
            // Reuse a successfully rotated authority credential after an uncertain
            // previous delivery without forcing another OAuth rotation.
            state.rejected = false;
            previous
        };
        self.resolve_credential(false, previous.as_deref())
            .await
            .map(drop)
    }
}
