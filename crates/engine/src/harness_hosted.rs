//! Zuse-hosted models: the cloud holds the upstream key and bills prepaid credits.
//! Requests carry the signed-in account token; there is never a fallback route.
use crate::{account::Account, harness_models::HarnessModel};
use futures::StreamExt;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    sync::Mutex,
    time::{Duration, Instant},
};
use zuse_harness::{
    Error,
    transport::{Authentication, Capabilities, Credential, Protocol, Route},
};

pub(crate) const PREFIX: &str = "zuse/";
const CATALOG_LIMIT: usize = 512 * 1024;

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HostedModel {
    id: String,
    label: String,
    context_window_tokens: u64,
    #[serde(default = "output_limit")]
    max_output_tokens: u32,
    #[serde(default)]
    tools: bool,
    #[serde(default)]
    images: bool,
    #[serde(default)]
    reasoning: bool,
}
fn output_limit() -> u32 {
    16384
}
#[derive(Deserialize)]
struct Catalog {
    models: Vec<HostedModel>,
}
#[derive(Default)]
struct Cached {
    subject: String,
    models: Vec<HostedModel>,
    loaded: Option<Instant>,
    error_until: Option<Instant>,
}

pub(crate) struct HostedModels {
    account: Option<Account>,
    origin: String,
    http: reqwest::Client,
    cache: tokio::sync::Mutex<Cached>,
    last: Mutex<Vec<HostedModel>>,
}
fn failure(code: &str, status: Option<u16>) -> Error {
    Error::Model {
        code: code.into(),
        status,
    }
}
impl HostedModels {
    pub fn new(account: Option<Account>) -> Self {
        let origin = std::env::var("ZUSE_CLOUD_API_URL")
            .unwrap_or_else(|_| crate::cloud::DEFAULT_API_URL.into());
        Self {
            account,
            origin: origin.trim_end_matches('/').into(),
            http: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .connect_timeout(Duration::from_secs(10))
                .timeout(Duration::from_secs(10))
                .build()
                .unwrap_or_default(),
            cache: Default::default(),
            last: Mutex::new(Vec::new()),
        }
    }
    #[cfg(test)]
    pub fn disabled() -> Self {
        Self::new(None)
    }
    async fn token(&self) -> Result<(String, String), Error> {
        let account = self.account.as_ref().ok_or(Error::AccountsUnavailable)?;
        let credential = account
            .credential()
            .await
            .map_err(|_| Error::AccountsUnavailable)?;
        Ok((credential.subject, credential.access_token))
    }
    /// Signed-out or unentitled accounts simply see no hosted models.
    pub async fn models(&self) -> Vec<HarnessModel> {
        self.catalog()
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|m| HarnessModel {
                id: format!("{PREFIX}{}", m.id),
                label: m.label,
                context_window_tokens: Some(m.context_window_tokens),
                auto_compact_token_limit: None,
                effective_context_window_percent: None,
            })
            .collect()
    }
    async fn catalog(&self) -> Result<Vec<HostedModel>, Error> {
        let (subject, token) = self.token().await?;
        let mut cache = self.cache.lock().await;
        if cache.subject == subject {
            if cache
                .loaded
                .is_some_and(|at| at.elapsed() < Duration::from_secs(15 * 60))
            {
                return Ok(cache.models.clone());
            }
            if cache.error_until.is_some_and(|at| at > Instant::now()) {
                return Err(failure("model_inventory_unavailable", None));
            }
        }
        *cache = Cached {
            subject,
            ..Default::default()
        };
        match self.load(&token).await {
            Ok(models) => {
                cache.loaded = Some(Instant::now());
                cache.models = models.clone();
                *self.last.lock().unwrap_or_else(|e| e.into_inner()) = models.clone();
                Ok(models)
            }
            Err(error) => {
                cache.error_until = Some(Instant::now() + Duration::from_secs(60));
                Err(error)
            }
        }
    }
    async fn load(&self, token: &str) -> Result<Vec<HostedModel>, Error> {
        let response = self
            .http
            .get(format!("{}/v1/models/catalog", self.origin))
            .bearer_auth(token)
            .send()
            .await
            .map_err(|_| failure("model_inventory_unavailable", None))?;
        if !response.status().is_success() {
            return Err(failure(
                "model_inventory_unavailable",
                Some(response.status().as_u16()),
            ));
        }
        let mut bytes = Vec::new();
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|_| failure("model_inventory_unavailable", None))?;
            if bytes.len() + chunk.len() > CATALOG_LIMIT {
                return Err(failure("invalid_model_inventory", None));
            }
            bytes.extend_from_slice(&chunk);
        }
        let catalog: Catalog =
            serde_json::from_slice(&bytes).map_err(|_| failure("invalid_model_inventory", None))?;
        Ok(catalog
            .models
            .into_iter()
            .filter(|m| {
                !m.id.is_empty()
                    && m.id.len() <= 256
                    && !m.id.chars().any(char::is_whitespace)
                    && m.context_window_tokens > 0
                    && (1..=1_000_000).contains(&m.max_output_tokens)
            })
            .take(512)
            .collect())
    }
    /// Stable per account so the broker keeps affinity without exposing the token.
    pub async fn connection(&self) -> Result<String, Error> {
        let (subject, _) = self.token().await?;
        Ok(format!("zuse/{:x}", Sha256::digest(subject.as_bytes())))
    }
    pub async fn credential(&self, model: &str) -> Result<(String, Credential), Error> {
        let id = model
            .strip_prefix(PREFIX)
            .filter(|id| !id.is_empty())
            .ok_or(Error::AccountsUnavailable)?;
        let (subject, token) = self.token().await?;
        let known = self
            .last
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .find(|m| m.id == id)
            .cloned();
        let model = match known {
            Some(model) => model,
            None => self
                .catalog()
                .await?
                .into_iter()
                .find(|m| m.id == id)
                .ok_or_else(|| failure("choose_a_zuse_model", None))?,
        };
        Ok((
            model.id.clone(),
            Credential {
                connection_id: format!("zuse/{:x}", Sha256::digest(subject.as_bytes())),
                authorization_generation: 0,
                access_token: token,
                route: Route {
                    endpoint: format!("{}/v1/models/chat/completions", self.origin),
                    protocol: Protocol::ChatCompletions,
                    subscription: false,
                    authentication: Authentication::Bearer,
                    betas: Vec::new(),
                    capabilities: Capabilities {
                        images: model.images,
                        tools: model.tools,
                        reasoning: model.reasoning,
                        prompt_cache: false,
                    },
                    max_output_tokens: model.max_output_tokens,
                    allowed_reasoning: None,
                    tool_namespace: None,
                    include_encrypted_reasoning: false,
                },
            },
        ))
    }
}
