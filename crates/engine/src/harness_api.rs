//! Explicit API routes, independent of subscription authorization.
use crate::harness_models::HarnessModel;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{collections::HashSet, path::Path};
use zuse_harness::{
    Error,
    transport::{Authentication, Capabilities, Credential, Protocol, Route},
};
#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ApiModels {
    #[serde(default)]
    connections: Vec<ApiConnection>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ApiConnection {
    id: String,
    protocol: Protocol,
    endpoint: String,
    #[serde(alias = "apiKeyEnv")]
    credential_env: String,
    #[serde(default)]
    authentication: Authentication,
    #[serde(default)]
    betas: Vec<String>,
    #[serde(default)]
    subscription: bool,
    capabilities: Capabilities,
    #[serde(default = "output_limit")]
    max_output_tokens: u32,
    models: Vec<ApiModel>,
    #[serde(default)]
    tool_namespace: Option<String>,
    #[serde(default)]
    include_encrypted_reasoning: bool,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ApiModel {
    id: String,
    label: String,
    context_window_tokens: u64,
}
fn output_limit() -> u32 {
    16384
}
fn error() -> Error {
    Error::paused("Invalid harness API configuration. Check harness/providers.json.")
}
impl ApiModels {
    pub fn load(path: &Path) -> Result<Self, Error> {
        let data = match std::fs::read(path) {
            Ok(data) => data,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(_) => return Err(error()),
        };
        if data.len() > 1024 * 1024 {
            return Err(error());
        }
        let config: Self = serde_json::from_slice(&data).map_err(|_| error())?;
        let mut ids = HashSet::new();
        if config.connections.len() > 128 {
            return Err(error());
        }
        for c in &config.connections {
            if c.id.is_empty()
                || c.id.len() > 64
                || !c
                    .id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
                || !ids.insert(&c.id)
                || c.models.len() > 2048
                || c.max_output_tokens == 0
                || c.max_output_tokens > 1_000_000
            {
                return Err(error());
            }
            let url = reqwest::Url::parse(&c.endpoint.replace("{model}", "model"))
                .map_err(|_| error())?;
            if url.scheme() != "https"
                || !url.username().is_empty()
                || url.password().is_some()
                || url.fragment().is_some()
            {
                return Err(error());
            }
            if c.credential_env.is_empty()
                || !c
                    .credential_env
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_')
            {
                return Err(error());
            }
            Route {
                endpoint: c.endpoint.clone(),
                protocol: c.protocol,
                subscription: c.subscription,
                authentication: c.authentication,
                betas: c.betas.clone(),
                capabilities: c.capabilities.clone(),
                max_output_tokens: c.max_output_tokens,
                tool_namespace: c.tool_namespace.clone(),
                include_encrypted_reasoning: c.include_encrypted_reasoning,
            }
            .validate_authentication()
            .map_err(|_| error())?;
            let mut models = HashSet::new();
            for model in &c.models {
                if model.id.is_empty()
                    || model.id.len() > 256
                    || model.id.chars().any(char::is_whitespace)
                    || !models.insert(&model.id)
                    || model.context_window_tokens == 0
                {
                    return Err(error());
                }
            }
        }
        Ok(config)
    }
    fn available(c: &ApiConnection) -> bool {
        std::env::var(&c.credential_env).is_ok_and(|s| !s.is_empty())
    }
    pub fn models(&self) -> Vec<HarnessModel> {
        self.connections
            .iter()
            .filter(|c| Self::available(c))
            .flat_map(|c| {
                c.models.iter().map(|m| HarnessModel {
                    id: format!("api/{}/{}", c.id, m.id),
                    label: m.label.clone(),
                    context_window_tokens: Some(m.context_window_tokens),
                    auto_compact_token_limit: None,
                    effective_context_window_percent: None,
                })
            })
            .collect()
    }
    pub fn connection(&self, model: &str) -> Result<String, Error> {
        let (c, _) = self.find(model)?;
        if !Self::available(c) {
            return Err(Error::AccountsUnavailable);
        }
        let key = std::env::var(&c.credential_env).map_err(|_| Error::AccountsUnavailable)?;
        Ok(format!("api/{}/{:x}", c.id, Sha256::digest(key.as_bytes())))
    }
    fn find(&self, model: &str) -> Result<(&ApiConnection, &ApiModel), Error> {
        let mut parts = model.splitn(3, '/');
        if parts.next() != Some("api") {
            return Err(Error::AccountsUnavailable);
        }
        let id = parts.next().ok_or(Error::AccountsUnavailable)?;
        let model = parts.next().ok_or(Error::AccountsUnavailable)?;
        let c = self
            .connections
            .iter()
            .find(|c| c.id == id)
            .ok_or(Error::AccountsUnavailable)?;
        let m = c
            .models
            .iter()
            .find(|m| m.id == model)
            .ok_or(Error::AccountsUnavailable)?;
        Ok((c, m))
    }
    pub fn credential(&self, model: &str) -> Result<(String, Credential), Error> {
        let (c, m) = self.find(model)?;
        let key = std::env::var(&c.credential_env).map_err(|_| Error::AccountsUnavailable)?;
        if key.is_empty() {
            return Err(Error::AccountsUnavailable);
        }
        let encoded =
            m.id.bytes()
                .map(|b| {
                    if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.') {
                        (b as char).to_string()
                    } else {
                        format!("%{b:02X}")
                    }
                })
                .collect::<String>();
        Ok((
            m.id.clone(),
            Credential {
                connection_id: format!("api/{}/{:x}", c.id, Sha256::digest(key.as_bytes())),
                authorization_generation: 0,
                access_token: key,
                route: Route {
                    endpoint: c.endpoint.replace("{model}", &encoded),
                    protocol: c.protocol,
                    subscription: c.subscription,
                    authentication: c.authentication,
                    betas: c.betas.clone(),
                    capabilities: c.capabilities.clone(),
                    max_output_tokens: c.max_output_tokens,
                    tool_namespace: c.tool_namespace.clone(),
                    include_encrypted_reasoning: c.include_encrypted_reasoning,
                },
            },
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn connection() -> serde_json::Value {
        json!({"id":"fixture","protocol":"responses","endpoint":"https://example.invalid/v1/responses","apiKeyEnv":"PATH","capabilities":{"images":true,"tools":true,"reasoning":true,"promptCache":true},"models":[{"id":"org/model","label":"Fixture","contextWindowTokens":32000}]})
    }
    fn load(value: serde_json::Value) -> Result<ApiModels, Error> {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("providers.json");
        std::fs::write(&path, value.to_string()).unwrap();
        ApiModels::load(&path)
    }
    #[test]
    fn configured_models_resolve_exact_routes_without_subscription_fallback() {
        // PATH is a nonsecret existing variable; no network request is made.
        let models = load(json!({"connections":[connection()]})).unwrap();
        assert_eq!(models.models()[0].id, "api/fixture/org/model");
        let (id, credential) = models.credential("api/fixture/org/model").unwrap();
        assert_eq!(id, "org/model");
        assert!(!credential.route.subscription);
        assert_eq!(
            credential.connection_id,
            models.connection("api/fixture/org/model").unwrap()
        );
        assert!(models.credential("api/unknown/org/model").is_err());
    }
    #[test]
    fn invalid_configuration_cannot_change_provider_or_authentication_routes() {
        assert!(load(json!({"connections":[connection(),connection()]})).is_err());
        let mut bad = connection();
        bad["endpoint"] = json!("http://example.invalid/");
        assert!(load(json!({"connections":[bad]})).is_err());
        let mut missing = connection();
        missing["apiKeyEnv"] = json!("ZUSE_TEST_NONEXISTENT_EVALUATION_CREDENTIAL");
        let models = load(json!({"connections":[missing]})).unwrap();
        assert!(models.models().is_empty());
        assert!(matches!(
            models.credential("api/fixture/org/model"),
            Err(Error::AccountsUnavailable)
        ));
    }
    #[test]
    fn explicit_bearer_connections_preserve_declared_subscription_billing() {
        let mut config = connection();
        config.as_object_mut().unwrap().remove("apiKeyEnv");
        config["credentialEnv"] = json!("PATH");
        config["authentication"] = json!("bearer");
        config["protocol"] = json!("messages");
        config["betas"] = json!(["oauth-2025-04-20"]);
        config["subscription"] = json!(true);
        let models = load(json!({"connections":[config]})).unwrap();
        let (_, credential) = models.credential("api/fixture/org/model").unwrap();
        assert!(credential.route.subscription);
        assert_eq!(credential.route.authentication, Authentication::Bearer);
        assert_eq!(credential.route.betas, ["oauth-2025-04-20"]);
    }
}
