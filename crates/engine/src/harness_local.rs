//! Local CLI catalogs and active logins for the Zuse agent loop.
use crate::{
    agent_accounts::AgentAccounts, harness_models::HarnessModel, registry::HarnessRegistry,
};
use sha2::{Digest, Sha256};
#[cfg(test)]
use std::sync::Arc;
use std::{sync::Weak, time::Duration};
use tokio::{sync::Mutex, time::Instant};
use zuse_harness::{
    Error,
    transport::{Authentication, Capabilities, Credential, Protocol, Route},
};
use zuse_protocol::HarnessId;

pub(crate) struct LocalModels {
    accounts: AgentAccounts,
    // The registry owns the native harness backed by these models.
    registry: Weak<HarnessRegistry>,
    inventories: [Mutex<Option<CachedInventory>>; 3],
}
const INVENTORY_TTL: Duration = Duration::from_secs(60);
struct CachedInventory {
    credential: [u8; 32],
    loaded: Instant,
    models: Vec<HarnessModel>,
}
const PROVIDERS: [(&str, HarnessId); 3] = [
    ("codex", HarnessId::Codex),
    ("claude", HarnessId::ClaudeCode),
    ("grok", HarnessId::Grok),
];
fn parse(id: &str) -> Result<(HarnessId, &str), Error> {
    let (provider, model) = id
        .strip_prefix("local/")
        .and_then(|s| s.split_once('/'))
        .ok_or(Error::AccountsUnavailable)?;
    let harness = PROVIDERS
        .iter()
        .find(|(name, _)| *name == provider)
        .map(|(_, id)| *id)
        .ok_or(Error::AccountsUnavailable)?;
    if model.is_empty() || model.chars().any(char::is_whitespace) {
        return Err(Error::AccountsUnavailable);
    }
    Ok((harness, model))
}
impl LocalModels {
    pub fn new(accounts: AgentAccounts, registry: Weak<HarnessRegistry>) -> Self {
        Self {
            accounts,
            registry,
            inventories: std::array::from_fn(|_| Mutex::new(None)),
        }
    }
    pub async fn models(&self, force: bool) -> Vec<HarnessModel> {
        let Some(registry) = self.registry.upgrade() else {
            return vec![];
        };
        let inventories = futures::future::join_all(PROVIDERS.into_iter().enumerate().map(
            |(index, (name, harness))| {
                let registry = &registry;
                async move {
                    // Serialize each provider's cold probe, including credential lookup,
                    // so concurrent starts reuse one completed inventory.
                    let mut slot = self.inventories[index].lock().await;
                    let Some(token) = self.accounts.harness_token(harness).await else {
                        *slot = None;
                        return vec![];
                    };
                    let credential: [u8; 32] = Sha256::digest(token.as_bytes()).into();
                    if !force
                        && let Some(cached) = slot.as_ref()
                        && cached.credential == credential
                        && cached.loaded.elapsed() < INVENTORY_TTL
                    {
                        return cached.models.clone();
                    }
                    let Ok(models) = registry.discover_models_force(harness, force).await else {
                        *slot = None;
                        return vec![];
                    };
                    let metadata = if harness == HarnessId::Codex {
                        self.codex_context_metadata().await
                    } else {
                        vec![]
                    };
                    let models = models
                        .into_iter()
                        .map(|model| local_model(name, model, &metadata))
                        .collect::<Vec<_>>();
                    *slot = Some(CachedInventory {
                        credential,
                        loaded: Instant::now(),
                        models: models.clone(),
                    });
                    models
                }
            },
        ))
        .await;
        inventories.into_iter().flatten().collect()
    }
    async fn codex_context_metadata(&self) -> Vec<HarnessModel> {
        let path = self.accounts.codex_models_cache_path();
        if !tokio::fs::metadata(&path)
            .await
            .is_ok_and(|m| m.len() <= 2 * 1024 * 1024)
        {
            return vec![];
        }
        let Ok(bytes) = tokio::fs::read(path).await else {
            return vec![];
        };
        serde_json::from_slice(&bytes)
            .ok()
            .and_then(|body| {
                crate::harness_models::inventory(zuse_model_accounts::Provider::Chatgpt, body).ok()
            })
            .unwrap_or_default()
    }
    pub async fn credential(&self, id: &str) -> Result<(String, Credential), Error> {
        let (harness, model) = parse(id)?;
        let mut token = self.accounts.harness_token(harness).await.ok_or_else(|| {
            Error::paused("Sign in with the selected local CLI, then refresh Zuse's models.")
        })?;
        if harness == HarnessId::Codex {
            use base64::Engine as _;
            let expires = token
                .split('.')
                .nth(1)
                .and_then(|s| {
                    base64::engine::general_purpose::URL_SAFE_NO_PAD
                        .decode(s)
                        .ok()
                })
                .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
                .and_then(|claims| claims["exp"].as_u64());
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            if expires.is_some_and(|exp| exp <= now + 60) {
                token = self
                    .accounts
                    .dictation_auth(Some(&token))
                    .await
                    .map_err(|_| Error::paused("Refresh your local Codex login and retry."))?
                    .0;
            }
        }
        // Bind provider-native reasoning to the active token, including account switches.
        let fingerprint = Sha256::digest(token.as_bytes());
        let connection_id = format!("local/{}/{:x}", id.split('/').nth(1).unwrap(), fingerprint);
        let (endpoint, protocol, authentication) = match harness {
            HarnessId::Codex => (
                "https://chatgpt.com/backend-api/codex/responses",
                Protocol::Responses,
                Authentication::CodexOauth,
            ),
            HarnessId::ClaudeCode => (
                "https://api.anthropic.com/v1/messages",
                Protocol::Messages,
                Authentication::AnthropicOauth,
            ),
            HarnessId::Grok => (
                "https://cli-chat-proxy.grok.com/v1/chat/completions",
                Protocol::ChatCompletions,
                Authentication::GrokOauth,
            ),
            _ => return Err(Error::AccountsUnavailable),
        };
        Ok((
            model.to_owned(),
            Credential {
                connection_id,
                access_token: token,
                authorization_generation: 0,
                route: Route {
                    endpoint: endpoint.into(),
                    protocol,
                    subscription: true,
                    authentication,
                    betas: vec![],
                    capabilities: Capabilities {
                        images: true,
                        tools: true,
                        reasoning: true,
                        prompt_cache: harness != HarnessId::Grok,
                    },
                    max_output_tokens: 16384,
                    allowed_reasoning: None,
                    tool_namespace: None,
                    include_encrypted_reasoning: harness == HarnessId::Codex,
                },
            },
        ))
    }
}

fn local_model(name: &str, model: zuse_protocol::Model, metadata: &[HarnessModel]) -> HarnessModel {
    let limits = metadata
        .iter()
        .find(|m| m.id.strip_prefix("chatgpt/") == Some(model.id.as_str()));
    HarnessModel {
        context_window_tokens: Some(
            limits
                .and_then(|m| m.context_window_tokens)
                .unwrap_or_else(|| zuse_harness::prompt::default_window(&model.id)),
        ),
        auto_compact_token_limit: limits.and_then(|m| m.auto_compact_token_limit),
        effective_context_window_percent: limits.and_then(|m| m.effective_context_window_percent),
        id: format!("local/{name}/{}", model.id),
        label: format!("{} · {name} (local)", model.label),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_accounts::AgentAccountsConfig;
    use serde_json::json;

    fn model(id: &str) -> zuse_protocol::Model {
        zuse_protocol::Model {
            id: id.into(),
            label: id.into(),
            description: None,
            reasoning_levels: vec![],
            options: vec![],
        }
    }

    #[test]
    fn local_catalog_keeps_model_windows_and_provider_compaction_metadata() {
        let metadata = crate::harness_models::inventory(
            zuse_model_accounts::Provider::Chatgpt,
            json!({
                "models": [{"slug":"gpt-6.1-sol", "visibility":"list", "context_window":750000,
                    "auto_compact_token_limit":650000, "effective_context_window_percent":96}]
            }),
        )
        .unwrap();
        let selected = local_model("codex", model("gpt-6.1-sol"), &metadata);
        assert_eq!(selected.id, "local/codex/gpt-6.1-sol");
        assert_eq!(selected.context_window_tokens, Some(750000));
        assert_eq!(selected.auto_compact_token_limit, Some(650000));
        assert_eq!(selected.effective_context_window_percent, Some(96.));
        assert_eq!(
            local_model("codex", model("gpt-6.1-sol"), &[]).context_window_tokens,
            Some(500000)
        );
        assert_eq!(
            local_model("claude", model("claude-opus-5-5"), &[]).context_window_tokens,
            Some(1000000)
        );
        assert_eq!(
            local_model("claude", model("claude-haiku-4-5"), &[]).context_window_tokens,
            Some(200000)
        );
    }

    #[tokio::test]
    async fn codex_context_metadata_uses_the_configured_cli_home() {
        let (_dir, config, local, _registry) = setup();
        let path = config.codex_home.join("models_cache.json");
        assert!(local.codex_context_metadata().await.is_empty());
        write(
            &path,
            json!({"models":[{"slug":"gpt-6.1-sol", "visibility":"list", "context_window":500000}]}),
        );
        let metadata = local.codex_context_metadata().await;
        assert_eq!(metadata.len(), 1);
        assert_eq!(metadata[0].context_window_tokens, Some(500000));
        std::fs::write(path, "invalid json").unwrap();
        assert!(local.codex_context_metadata().await.is_empty());
    }

    fn setup() -> (
        tempfile::TempDir,
        AgentAccountsConfig,
        LocalModels,
        Arc<HarnessRegistry>,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let config = AgentAccountsConfig::isolated(dir.path());
        let registry = Arc::new(HarnessRegistry::new());
        let local = LocalModels::new(
            AgentAccounts::new(config.clone()),
            Arc::downgrade(&registry),
        );
        (dir, config, local, registry)
    }
    fn write(path: &std::path::Path, body: serde_json::Value) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, serde_json::to_vec(&body).unwrap()).unwrap();
    }
    #[tokio::test]
    async fn local_credentials_route_each_provider_and_follow_account_changes() {
        let (_dir, config, local, _registry) = setup();
        write(
            &config.codex_home.join("auth.json"),
            json!({"tokens":{"access_token":"codex-one"}}),
        );
        write(
            &config.claude_config_dir.join(".credentials.json"),
            json!({"claudeAiOauth":{"accessToken":"claude-one"}}),
        );
        write(
            &config.grok_home.join("auth.json"),
            json!({"https://auth.x.ai::client":{"key":"grok-one"}}),
        );
        for (name, token, endpoint, protocol) in [
            (
                "codex",
                "codex-one",
                "https://chatgpt.com/backend-api/codex/responses",
                Protocol::Responses,
            ),
            (
                "claude",
                "claude-one",
                "https://api.anthropic.com/v1/messages",
                Protocol::Messages,
            ),
            (
                "grok",
                "grok-one",
                "https://cli-chat-proxy.grok.com/v1/chat/completions",
                Protocol::ChatCompletions,
            ),
        ] {
            let (model, credential) = local
                .credential(&format!("local/{name}/test-model"))
                .await
                .unwrap();
            assert_eq!(model, "test-model");
            assert_eq!(credential.access_token, token);
            assert_eq!(credential.route.endpoint, endpoint);
            assert_eq!(credential.route.protocol, protocol);
            assert!(!credential.connection_id.contains(token));
        }
        let (_, before) = local.credential("local/codex/test-model").await.unwrap();
        write(
            &config.codex_home.join("auth.json"),
            json!({"tokens":{"access_token":"codex-two"}}),
        );
        let (_, after) = local.credential("local/codex/test-model").await.unwrap();
        assert_ne!(before.connection_id, after.connection_id);
        assert_eq!(after.access_token, "codex-two");
        std::fs::remove_file(config.codex_home.join("auth.json")).unwrap();
        assert!(local.credential("local/codex/test-model").await.is_err());
    }
    #[tokio::test]
    async fn missing_login_and_enterprise_grok_are_not_public_api_credentials() {
        let (_dir, config, local, _registry) = setup();
        assert!(local.models(false).await.is_empty());
        assert!(local.credential("local/claude/test-model").await.is_err());
        write(
            &config.grok_home.join("auth.json"),
            json!({"https://enterprise.example::client":{"key":"enterprise-token"}}),
        );
        assert!(local.credential("local/grok/test-model").await.is_err());
        write(
            &config.grok_home.join("auth.json"),
            json!({"https://accounts.x.ai/sign-in":{"key":"legacy-cli-token"}}),
        );
        assert_eq!(
            local
                .credential("local/grok/test-model")
                .await
                .unwrap()
                .1
                .access_token,
            "legacy-cli-token"
        );
        for invalid in [
            "local/unknown/model",
            "local/codex/",
            "local/claude/bad model",
        ] {
            assert!(local.credential(invalid).await.is_err());
        }
    }
}

/// Explicit live smoke test. Reads the selected CLI's active login and sends one
/// small prompt; never prints credentials or provider response bodies.
#[cfg(test)]
mod live_tests {
    use super::*;
    use zuse_harness::{
        Message, ModelInput, RequestBudget, Role, Tools,
        transport::{ModelTransport, NativeTransport},
    };
    struct Budget;
    #[async_trait::async_trait]
    impl RequestBudget for Budget {
        async fn reserve(&self) -> Result<(), Error> {
            Ok(())
        }
    }

    #[tokio::test]
    #[ignore = "sends a live subscription request using local CLI authentication"]
    async fn live_local_subscription() {
        let provider = std::env::var("ZUSE_LIVE_PROVIDER").unwrap_or_else(|_| "claude".into());
        let harness = PROVIDERS
            .iter()
            .find(|(name, _)| *name == provider)
            .map(|(_, harness)| *harness)
            .expect("choose codex, claude, or grok");
        let profile = tempfile::tempdir().unwrap();
        let accounts = AgentAccounts::new(crate::agent_accounts::AgentAccountsConfig::detect(
            profile.path(),
        ));
        assert!(
            accounts.harness_token(harness).await.is_some(),
            "No active local login for {provider}"
        );
        let registry = Arc::new(crate::registry::default_registry());
        let local = LocalModels::new(accounts, Arc::downgrade(&registry));
        let model = match std::env::var("ZUSE_LIVE_MODEL") {
            Ok(model) => model,
            Err(_) => registry
                .discover_models_force(harness, true)
                .await
                .expect("local CLI model discovery failed")
                .first()
                .expect("empty local catalog")
                .id
                .clone(),
        };
        let (model, mut credential) = local
            .credential(&format!("local/{provider}/{model}"))
            .await
            .expect("local credentials unavailable");
        credential.route.max_output_tokens = 64;
        let input = ModelInput {
            root_id: "local-subscription-smoke".into(),
            agent_id: "root".into(),
            model,
            reasoning: None,
            messages: Arc::new(vec![Message::text(Role::User, "Reply with exactly OK.")]),
            tools: Arc::new(Tools::new()),
            purpose: Default::default(),
            retry_index: 0,
            cancel: tokio_util::sync::CancellationToken::new(),
            text: Arc::new(|_| {}),
            thinking: Arc::new(|_| {}),
            budget: Arc::new(Budget),
            report: Arc::new(|_| {}),
        };
        let transport = NativeTransport::new().unwrap();
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(60),
            transport.request(input, credential),
        )
        .await;
        transport.close().await;
        let output = result
            .expect("live request timed out")
            .expect("live subscription request failed");
        assert!(!output.text.trim().is_empty(), "empty live response");
        println!("{provider}: live subscription request succeeded");
    }
}
