//! Workload credentials are sealed to this runtime and remain in process memory.
//! The recipe contains reference IDs only, never the values supplied here.
use crate::{Authority, Identity, now_ms};
use anyhow::{Result, ensure};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD as B64};
use rsa::RsaPrivateKey;
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, sync::Arc};
use tokio::sync::{Mutex, watch};
use zeroize::Zeroizing;
use zuse_protocol::cloud::CloudRuntimeBootstrap;
pub type Environment = BTreeMap<String, Zeroizing<String>>;
struct Secret(Zeroizing<String>);
impl<'de> Deserialize<'de> for Secret {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        Ok(Self(Zeroizing::new(String::deserialize(deserializer)?)))
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Plain {
    protocol_version: u32,
    request_id: String,
    workspace_id: String,
    owner_id: String,
    runtime_generation: i64,
    credential_key_thumbprint: String,
    expires_at_ms: i64,
    bindings: BTreeMap<String, i64>,
    environment: BTreeMap<String, Secret>,
}
pub struct Recipient {
    key: RsaPrivateKey,
    public: String,
    thumb: String,
    workspace: String,
    owner: String,
    generation: i64,
}
impl Recipient {
    pub(crate) fn new(
        bootstrap: &CloudRuntimeBootstrap,
        key: RsaPrivateKey,
        public: String,
    ) -> Result<Self> {
        let jwk: Value = serde_json::from_str(&public)?;
        let thumb = B64.encode(Sha256::digest(serde_json::to_vec(
            &json!({"e":jwk["e"],"kty":"RSA","n":jwk["n"]}),
        )?));
        Ok(Self {
            key,
            public,
            thumb,
            workspace: bootstrap.workspace_id.clone(),
            owner: bootstrap.zuse_account_id.clone(),
            generation: bootstrap.runtime_generation,
        })
    }
    fn open(
        &self,
        response: &Value,
        request: &str,
        bindings: &[String],
    ) -> Result<(Arc<Environment>, i64)> {
        ensure!(
            response["protocolVersion"] == 1
                && response["requestId"] == request
                && response["runtimeGeneration"] == self.generation,
            "workload_grant_binding_invalid"
        );
        let raw = crate::github::open_secret(
            &self.key,
            response["sealedGrant"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("workload_grant_invalid"))?,
        )?;
        let value: Plain =
            serde_json::from_slice(&raw).map_err(|_| anyhow::anyhow!("workload_grant_invalid"))?;
        ensure!(
            value.protocol_version == 1
                && value.request_id == request
                && value.workspace_id == self.workspace
                && value.owner_id == self.owner
                && value.runtime_generation == self.generation
                && value.credential_key_thumbprint == self.thumb
                && value.expires_at_ms > now_ms() + 5000
                && value.expires_at_ms <= now_ms() + 15 * 60_000
                && response["expiresAtMs"] == value.expires_at_ms
                && value.bindings.len() == bindings.len()
                && bindings
                    .iter()
                    .all(|id| value.bindings.get(id).is_some_and(|version| *version > 0)),
            "workload_grant_binding_invalid"
        );
        ensure!(
            value.environment.len() <= 64
                && value.environment.values().map(|v| v.0.len()).sum::<usize>() <= 24 * 1024
                && value
                    .environment
                    .iter()
                    .all(|(name, value)| valid_name(name)
                        && !value.0.is_empty()
                        && value.0.len() <= 8192
                        && !value.0.contains('\0')),
            "workload_grant_invalid"
        );
        Ok((
            Arc::new(
                value
                    .environment
                    .into_iter()
                    .map(|(name, value)| (name, value.0))
                    .collect(),
            ),
            value.expires_at_ms,
        ))
    }
}
fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && name
            .bytes()
            .enumerate()
            .all(|(i, b)| b == b'_' || b.is_ascii_uppercase() || (i > 0 && b.is_ascii_digit()))
        && !name.starts_with("ZUSE_")
        && !name.starts_with("DYLD_")
        && !name.starts_with("LD_")
        && ![
            "PATH",
            "HOME",
            "SHELL",
            "USER",
            "LOGNAME",
            "TMPDIR",
            "BASH_ENV",
            "ENV",
            "ANTHROPIC_API_KEY",
            "ANTHROPIC_AUTH_TOKEN",
            "CLAUDE_CODE_OAUTH_TOKEN",
            "OPENAI_API_KEY",
            "CODEX_API_KEY",
            "CODEX_ACCESS_TOKEN",
            "XAI_API_KEY",
            "GROK_API_KEY",
            "GROK_AUTH_PROVIDER_COMMAND",
            "CLOUD_DATA_ENCRYPTION_KEY",
            "CLOUD_CREDENTIAL_VAULT_KEY",
            "WORKOS_API_KEY",
        ]
        .contains(&name)
        // Every cloud-computer provider key, so a new provider is reserved by default.
        && !zuse_protocol::cloud_computers::ProviderId::is_api_key_env(name)
}
pub struct Broker {
    command_path: std::sync::OnceLock<std::path::PathBuf>,
    #[cfg(feature = "engine")]
    secret_filter: std::sync::OnceLock<Arc<zuse_providers::credentials::redaction::Secrets>>,
    recipient: Recipient,
    authority: Authority,
    updates: watch::Receiver<CloudRuntimeBootstrap>,
    bindings: Vec<String>,
    values: Mutex<Option<(Arc<Environment>, i64)>>,
    #[cfg(feature = "engine")]
    terminals: std::sync::OnceLock<zuse_engine::Terminals>,
}
impl Broker {
    pub fn new(
        identity: &Identity,
        bootstrap: &CloudRuntimeBootstrap,
        authority: Authority,
        updates: watch::Receiver<CloudRuntimeBootstrap>,
        bindings: Vec<String>,
    ) -> Result<Arc<Self>> {
        Self::from_recipient(
            identity.workload_recipient(bootstrap)?,
            bootstrap,
            authority,
            updates,
            bindings,
        )
    }
    pub fn from_recipient(
        recipient: Recipient,
        bootstrap: &CloudRuntimeBootstrap,
        authority: Authority,
        updates: watch::Receiver<CloudRuntimeBootstrap>,
        bindings: Vec<String>,
    ) -> Result<Arc<Self>> {
        ensure!(
            bindings.len() <= 64
                && (bindings.is_empty()
                    || bootstrap.extra.get("rustWorkloadSecretsVersion") == Some(&json!(1))),
            "workload_secret_runtime_update_required"
        );
        Ok(Arc::new(Self {
            #[cfg(feature = "engine")]
            secret_filter: std::sync::OnceLock::new(),
            recipient,
            authority,
            updates,
            bindings,
            command_path: std::sync::OnceLock::new(),
            values: Mutex::new(None),
            #[cfg(feature = "engine")]
            terminals: std::sync::OnceLock::new(),
        }))
    }
    #[cfg(feature = "engine")]
    pub fn set_secret_filter(
        &self,
        filter: Arc<zuse_providers::credentials::redaction::Secrets>,
    ) -> Result<()> {
        self.secret_filter
            .set(filter)
            .map_err(|_| anyhow::anyhow!("runtime_secret_filter_already_configured"))
    }
    /// Task-local non-secret PATH, configured once before environment delivery.
    pub fn set_command_path(&self, path: std::path::PathBuf) -> Result<()> {
        ensure!(
            path.is_absolute() && path.is_dir(),
            "runtime_command_path_invalid"
        );
        self.command_path
            .set(path)
            .map_err(|_| anyhow::anyhow!("runtime_command_path_already_configured"))
    }
    fn local_environment(&self, values: Arc<Environment>) -> Result<Arc<Environment>> {
        let Some(prefix) = self.command_path.get() else {
            return Ok(values);
        };
        let mut merged = values
            .iter()
            .map(|(key, value)| (key.clone(), Zeroizing::new(value.to_string())))
            .collect::<Environment>();
        let tail = merged
            .get("PATH")
            .map(|s| std::ffi::OsString::from(s.as_str()))
            .unwrap_or_else(|| std::env::var_os("PATH").unwrap_or_default());
        let path = std::env::join_paths(
            std::iter::once(prefix.clone()).chain(std::env::split_paths(&tail)),
        )?;
        merged.insert(
            "PATH".into(),
            Zeroizing::new(
                path.to_str()
                    .ok_or_else(|| anyhow::anyhow!("runtime_command_path_invalid"))?
                    .into(),
            ),
        );
        Ok(Arc::new(merged))
    }
    pub async fn values(&self, fresh: bool) -> Result<Arc<Environment>> {
        if self.bindings.is_empty() {
            return self.local_environment(Arc::new(BTreeMap::new()));
        }
        let mut cached = self.values.lock().await;
        if !fresh {
            if let Some((value, expiry)) = &*cached {
                if *expiry > now_ms() + 30000 {
                    return Ok(value.clone());
                }
            }
        }
        let bootstrap = self.updates.borrow().clone();
        ensure!(
            bootstrap.workspace_id == self.recipient.workspace
                && bootstrap.runtime_generation == self.recipient.generation,
            "workload_runtime_fenced"
        );
        let request = uuid::Uuid::new_v4().to_string();
        let result=self.authority.post(&bootstrap.workspace_id,&["runtime","workload-secrets","grant"],&bootstrap.runtime_credential,&json!({"protocolVersion":1,"requestId":request,"runtimeGeneration":self.recipient.generation,"credentialPublicJwk":self.recipient.public,"bindings":self.bindings})).await;
        let result =
            result.and_then(|response| self.recipient.open(&response, &request, &self.bindings));
        match result {
            Ok((values, expiry)) => {
                #[cfg(feature = "engine")]
                if let Some(filter) = self.secret_filter.get() {
                    for value in values.values() {
                        filter.record(value)?;
                    }
                }
                let values = self.local_environment(values)?;
                #[cfg(feature = "engine")]
                if let Some(terminals) = self.terminals.get() {
                    terminals.set_host_environment(values.clone());
                }
                *cached = Some((values.clone(), expiry));
                Ok(values)
            }
            Err(error) => {
                *cached = None;
                #[cfg(feature = "engine")]
                if let Some(terminals) = self.terminals.get() {
                    terminals.set_host_environment(Arc::new(BTreeMap::new()));
                }
                Err(error)
            }
        }
    }
    #[cfg(feature = "engine")]
    pub async fn attach_terminals(&self, terminals: zuse_engine::Terminals) -> Result<()> {
        let values = self.values(false).await?;
        terminals.set_host_environment(values);
        let _ = self.terminals.set(terminals);
        Ok(())
    }
}
#[cfg(feature = "engine")]
pub struct Credentials {
    agent: Arc<dyn zuse_providers::credentials::CredentialResolver>,
    workload: Arc<Broker>,
}
#[cfg(feature = "engine")]
impl Credentials {
    pub fn wrap(
        agent: Arc<dyn zuse_providers::credentials::CredentialResolver>,
        workload: Arc<Broker>,
    ) -> Arc<Self> {
        Arc::new(Self { agent, workload })
    }
}
#[cfg(feature = "engine")]
#[async_trait::async_trait]
impl zuse_providers::credentials::CredentialResolver for Credentials {
    async fn environment(
        &self,
    ) -> std::result::Result<
        zuse_providers::credentials::ChildEnvironment,
        zuse_providers::HarnessError,
    > {
        let values = self.workload.values(false).await.map_err(|_| {
            zuse_providers::HarnessError::Protocol("workload-secrets-require-reconnection".into())
        })?;
        Ok(self.agent.environment().await?.with_workload(
            values
                .iter()
                .map(|(name, value)| (name.clone(), Zeroizing::new(value.to_string()))),
        ))
    }
    async fn before_turn(
        &self,
    ) -> std::result::Result<
        zuse_providers::credentials::ChildEnvironment,
        zuse_providers::HarnessError,
    > {
        let values = self.workload.values(true).await.map_err(|_| {
            zuse_providers::HarnessError::Protocol("workload-secrets-require-reconnection".into())
        })?;
        Ok(self.agent.before_turn().await?.with_workload(
            values
                .iter()
                .map(|(name, value)| (name.clone(), Zeroizing::new(value.to_string()))),
        ))
    }
}

#[cfg(feature = "engine")]
pub struct CodexCredentials {
    agent: Arc<dyn zuse_providers::codex::auth::CredentialResolver>,
    workload: Arc<Broker>,
    fingerprint: std::sync::Mutex<Option<Vec<u8>>>,
}
#[cfg(feature = "engine")]
impl CodexCredentials {
    pub fn wrap(
        agent: Arc<dyn zuse_providers::codex::auth::CredentialResolver>,
        workload: Arc<Broker>,
    ) -> Arc<Self> {
        Arc::new(Self {
            agent,
            workload,
            fingerprint: std::sync::Mutex::new(None),
        })
    }
}
#[cfg(feature = "engine")]
fn fingerprint(values: &Environment) -> Vec<u8> {
    let mut digest = Sha256::new();
    for (name, value) in values {
        digest.update((name.len() as u64).to_be_bytes());
        digest.update(name.as_bytes());
        digest.update((value.len() as u64).to_be_bytes());
        digest.update(value.as_bytes());
    }
    digest.finalize().to_vec()
}
#[cfg(feature = "engine")]
#[async_trait::async_trait]
impl zuse_providers::codex::auth::CredentialResolver for CodexCredentials {
    async fn environment(
        &self,
    ) -> std::result::Result<
        zuse_providers::credentials::ChildEnvironment,
        zuse_providers::HarnessError,
    > {
        let values = self.workload.values(false).await.map_err(|_| {
            zuse_providers::HarnessError::Protocol("workload-secrets-require-reconnection".into())
        })?;
        *self
            .fingerprint
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(fingerprint(&values));
        Ok(
            zuse_providers::credentials::ChildEnvironment::empty().with_workload(
                values
                    .iter()
                    .map(|(name, value)| (name.clone(), Zeroizing::new(value.to_string()))),
            ),
        )
    }
    async fn resolve(
        &self,
        unauthorized: bool,
        previous_account: Option<&str>,
    ) -> std::result::Result<zuse_providers::codex::auth::Credential, zuse_providers::HarnessError>
    {
        self.agent.resolve(unauthorized, previous_account).await
    }
    async fn before_turn(
        &self,
    ) -> std::result::Result<zuse_providers::codex::auth::Credential, zuse_providers::HarnessError>
    {
        let values = self.workload.values(true).await.map_err(|_| {
            zuse_providers::HarnessError::Protocol("workload-secrets-require-reconnection".into())
        })?;
        let changed = self
            .fingerprint
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .is_some_and(|prior| *prior != fingerprint(&values));
        if changed {
            return Err(zuse_providers::HarnessError::Protocol(
                "workload-secrets-require-runtime-restart".into(),
            ));
        }
        self.agent.before_turn().await
    }
}

#[cfg(test)]
mod tests;
