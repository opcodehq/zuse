//! Explicit API routes, independent of subscription authorization: entries from
//! `providers.json` plus the built-in Kimi and GLM key connections.
use crate::{
    harness_models::HarnessModel,
    harness_vendors::{self, ApiKeys},
};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{collections::HashSet, path::Path};
use zeroize::Zeroizing;
use zuse_harness::{
    Error,
    transport::{Authentication, Capabilities, Credential, Protocol, Route},
};
#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ApiModels {
    #[serde(default)]
    connections: Vec<ApiConnection>,
    #[serde(skip)]
    keys: ApiKeys,
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
    #[serde(default)]
    allowed_reasoning: Option<Vec<String>>,
    /// Built-in vendor connection: a saved key comes before the variable.
    #[serde(skip)]
    vendor: bool,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ApiModel {
    id: String,
    label: String,
    context_window_tokens: u64,
    #[serde(skip)]
    images: Option<bool>,
}
fn output_limit() -> u32 {
    16384
}
fn error() -> Error {
    Error::paused("Invalid harness API configuration. Check harness/providers.json.")
}
impl ApiModels {
    /// Configured connections keep their ids; a built-in connection is added
    /// for every vendor whose id the configuration does not already use.
    pub fn load(path: &Path, keys: ApiKeys) -> Result<Self, Error> {
        let mut config = Self::configured(path)?;
        for vendor in harness_vendors::VENDORS {
            if config.connections.iter().any(|c| c.id == vendor.id) {
                continue;
            }
            config.connections.push(ApiConnection {
                id: vendor.id.into(),
                protocol: Protocol::Messages,
                endpoint: vendor.endpoint.into(),
                credential_env: vendor.credential_env.into(),
                authentication: vendor.authentication,
                betas: vec![],
                subscription: vendor.subscription,
                capabilities: Capabilities {
                    images: true,
                    tools: true,
                    reasoning: true,
                    prompt_cache: true,
                },
                max_output_tokens: harness_vendors::MAX_OUTPUT_TOKENS,
                models: vendor
                    .models
                    .iter()
                    .map(|m| ApiModel {
                        id: m.id.into(),
                        label: m.label.into(),
                        context_window_tokens: m.context_window_tokens,
                        images: Some(m.images),
                    })
                    .collect(),
                tool_namespace: None,
                include_encrypted_reasoning: false,
                allowed_reasoning: None,
                vendor: true,
            });
        }
        config.keys = keys;
        Ok(config)
    }
    pub fn keys(&self) -> ApiKeys {
        self.keys.clone()
    }
    fn configured(path: &Path) -> Result<Self, Error> {
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
                allowed_reasoning: c.allowed_reasoning.clone(),
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
    fn key(&self, c: &ApiConnection) -> Option<Zeroizing<String>> {
        if c.vendor
            && let Some(key) = harness_vendors::vendor(&c.id).and_then(|v| self.keys.key(v))
        {
            return Some(key);
        }
        std::env::var(&c.credential_env)
            .ok()
            .filter(|s| !s.is_empty())
            .map(Zeroizing::new)
    }
    pub fn models(&self) -> Vec<HarnessModel> {
        self.connections
            .iter()
            .filter(|c| self.key(c).is_some())
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
        let key = self.key(c).ok_or(Error::AccountsUnavailable)?;
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
        let key = self.key(c).ok_or(Error::AccountsUnavailable)?;
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
                access_token: key.to_string(),
                route: Route {
                    endpoint: c.endpoint.replace("{model}", &encoded),
                    protocol: c.protocol,
                    subscription: c.subscription,
                    authentication: c.authentication,
                    betas: c.betas.clone(),
                    capabilities: Capabilities {
                        images: m.images.unwrap_or(c.capabilities.images),
                        ..c.capabilities.clone()
                    },
                    max_output_tokens: c.max_output_tokens,
                    allowed_reasoning: c.allowed_reasoning.clone(),
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
        ApiModels::load(&path, ApiKeys::default())
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
        assert!(
            !models
                .models()
                .iter()
                .any(|m| m.id.starts_with("api/fixture/"))
        );
        assert!(matches!(
            models.credential("api/fixture/org/model"),
            Err(Error::AccountsUnavailable)
        ));
    }
    fn keyed(path: &Path) -> ApiKeys {
        let keys = ApiKeys::open(path.join("api-keys.json"));
        keys.set("kimi", "sk-kimi-fixture").unwrap();
        keys.set("glm", "zai-fixture").unwrap();
        keys
    }
    #[test]
    fn built_in_vendor_models_appear_only_with_a_saved_key() {
        let directory = tempfile::tempdir().unwrap();
        let missing = directory.path().join("providers.json");
        let keys = ApiKeys::open(directory.path().join("api-keys.json"));
        let models = ApiModels::load(&missing, keys.clone()).unwrap();
        if std::env::var_os("MOONSHOT_API_KEY").is_none() {
            assert!(
                !models
                    .models()
                    .iter()
                    .any(|m| m.id.starts_with("api/kimi/"))
            );
            assert!(models.credential("api/kimi/kimi-k3.1-pro").is_err());
        }
        keys.set("kimi", "sk-kimi-fixture").unwrap();
        let listed = models.models();
        let pro = listed
            .iter()
            .find(|m| m.id == "api/kimi/kimi-k3.1-pro")
            .unwrap();
        assert_eq!(pro.label, "Kimi K3.1 Pro");
        assert_eq!(pro.context_window_tokens, Some(1_048_576));
        let (model, credential) = models.credential("api/kimi/kimi-k3.1-pro").unwrap();
        assert_eq!(model, "kimi-k3.1-pro");
        assert_eq!(credential.access_token, "sk-kimi-fixture");
        assert_eq!(
            credential.route.endpoint,
            "https://api.moonshot.ai/anthropic/v1/messages"
        );
        assert_eq!(credential.route.protocol, Protocol::Messages);
        assert_eq!(credential.route.authentication, Authentication::Bearer);
        assert!(!credential.route.subscription);
        assert!(credential.route.capabilities.tools && credential.route.capabilities.images);
        assert!(!credential.connection_id.contains("sk-kimi-fixture"));
        assert_eq!(
            credential.connection_id,
            models.connection("api/kimi/kimi-k3.1-pro").unwrap()
        );
        let before = credential.connection_id;
        keys.set("kimi", "sk-kimi-rotated").unwrap();
        assert_ne!(
            models.connection("api/kimi/kimi-k3.1-pro").unwrap(),
            before,
            "a new key must not reuse provider continuation state"
        );
    }
    #[test]
    fn glm_and_coding_plan_routes_match_vendor_endpoints() {
        let directory = tempfile::tempdir().unwrap();
        let keys = keyed(directory.path());
        keys.set("kimi-code", "sk-kimi-code-fixture").unwrap();
        let models = ApiModels::load(&directory.path().join("providers.json"), keys).unwrap();
        let (model, glm) = models.credential("api/glm/glm-5.3").unwrap();
        assert_eq!(model, "glm-5.3");
        assert_eq!(
            glm.route.endpoint,
            "https://api.z.ai/api/anthropic/v1/messages"
        );
        assert_eq!(glm.route.authentication, Authentication::Bearer);
        assert!(!glm.route.capabilities.images, "GLM-5.3 is text-only");
        let (_, flash) = models.credential("api/glm/glm-5.3-flash").unwrap();
        assert!(flash.route.capabilities.images);
        let (model, code) = models.credential("api/kimi-code/k3.1-pro").unwrap();
        assert_eq!(model, "k3.1-pro");
        assert_eq!(
            code.route.endpoint,
            "https://api.kimi.ai/coding/v1/messages"
        );
        assert_eq!(code.route.authentication, Authentication::ProtocolDefault);
        assert!(code.route.subscription);
        for (_, credential) in [
            models.credential("api/glm/glm-5.4").unwrap(),
            models.credential("api/kimi-code/k3").unwrap(),
        ] {
            credential.route.validate_authentication().unwrap();
        }
    }
    /// Replays a recorded-shape Anthropic Messages stream (thinking with a
    /// signature, then a tool call) from loopback for one vendor route, and
    /// returns the request headers and body the transport sent.
    async fn replay(
        mut credential: Credential,
        model: String,
        reasoning: Option<&str>,
    ) -> (zuse_harness::ModelOutput, String, serde_json::Value) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        credential.route.endpoint =
            format!("http://{}/v1/messages", listener.local_addr().unwrap());
        let events = [
            json!({"type":"message_start","message":{"id":"msg_1","model":model,"usage":{"input_tokens":12,"cache_read_input_tokens":30,"cache_creation_input_tokens":4}}}),
            json!({"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}}),
            json!({"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"Read the file first."}}),
            json!({"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"sig-1"}}),
            json!({"type":"content_block_stop","index":0}),
            json!({"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"call_1","name":"edit","input":{}}}),
            json!({"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"path\":\"src/main.rs\"}"}}),
            json!({"type":"content_block_stop","index":1}),
            json!({"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":9}}),
            json!({"type":"message_stop"}),
        ];
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut bytes = Vec::new();
            let mut chunk = [0; 8192];
            let boundary = loop {
                let n = socket.read(&mut chunk).await.unwrap();
                assert!(n > 0);
                bytes.extend_from_slice(&chunk[..n]);
                if let Some(i) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                    break i + 4;
                }
            };
            let headers = String::from_utf8(bytes[..boundary].to_vec())
                .unwrap()
                .to_ascii_lowercase();
            let length: usize = headers
                .lines()
                .find_map(|l| l.strip_prefix("content-length:"))
                .unwrap()
                .trim()
                .parse()
                .unwrap();
            while bytes.len() - boundary < length {
                let n = socket.read(&mut chunk).await.unwrap();
                assert!(n > 0);
                bytes.extend_from_slice(&chunk[..n]);
            }
            let body: serde_json::Value =
                serde_json::from_slice(&bytes[boundary..boundary + length]).unwrap();
            let stream: String = events
                .iter()
                .map(|e| format!("data: {e}\r\n\r\n"))
                .collect();
            socket
                .write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{stream}", stream.len()).as_bytes())
                .await
                .unwrap();
            (headers, body)
        });
        struct Budget;
        #[async_trait::async_trait]
        impl zuse_harness::RequestBudget for Budget {
            async fn reserve(&self) -> Result<(), Error> {
                Ok(())
            }
        }
        let input = zuse_harness::ModelInput {
            root_id: "root".into(),
            agent_id: "root".into(),
            model,
            reasoning: reasoning.map(str::to_owned),
            messages: std::sync::Arc::new(vec![
                zuse_harness::Message::text(zuse_harness::Role::System, "You are a coding agent."),
                zuse_harness::Message::text(zuse_harness::Role::User, "Fix the build."),
            ]),
            tools: std::sync::Arc::new(zuse_harness::Tools::from([(
                "edit".into(),
                zuse_harness::ToolDefinition {
                    description: "Edit a file".into(),
                    parameters: json!({"type":"object","properties":{"path":{"type":"string"}}}),
                    category: zuse_harness::ToolCategory::Edit,
                },
            )])),
            purpose: Default::default(),
            retry_index: 0,
            cancel: tokio_util::sync::CancellationToken::new(),
            text: std::sync::Arc::new(|_| {}),
            thinking: std::sync::Arc::new(|_| {}),
            budget: std::sync::Arc::new(Budget),
            report: std::sync::Arc::new(|_| {}),
        };
        use zuse_harness::transport::ModelTransport;
        let output = zuse_harness::transport::NativeTransport::new()
            .unwrap()
            .request(input, credential)
            .await
            .unwrap();
        let (headers, body) = server.await.unwrap();
        (output, headers, body)
    }
    #[tokio::test]
    async fn vendor_routes_send_messages_requests_and_parse_tool_calls() {
        let directory = tempfile::tempdir().unwrap();
        let keys = keyed(directory.path());
        keys.set("kimi-code", "sk-kimi-code-fixture").unwrap();
        let models = ApiModels::load(&directory.path().join("providers.json"), keys).unwrap();
        for (id, token, bearer) in [
            ("api/kimi/kimi-k3.1-pro", "sk-kimi-fixture", true),
            ("api/glm/glm-5.3", "zai-fixture", true),
            ("api/kimi-code/k3.1-pro", "sk-kimi-code-fixture", false),
        ] {
            let (model, credential) = models.credential(id).unwrap();
            let (output, headers, body) = replay(credential, model.clone(), Some("high")).await;
            if bearer {
                assert!(
                    headers.contains(&format!("authorization: bearer {token}\r\n")),
                    "{id}"
                );
                assert!(!headers.contains("x-api-key:"), "{id}");
            } else {
                assert!(headers.contains(&format!("x-api-key: {token}\r\n")), "{id}");
                assert!(!headers.contains("authorization:"), "{id}");
            }
            assert!(headers.contains("anthropic-version: 2023-06-01"), "{id}");
            assert!(!headers.contains("anthropic-beta"), "{id}");
            assert_eq!(body["model"], model, "{id}");
            assert_eq!(body["max_tokens"], 32768, "{id}");
            assert_eq!(body["stream"], true, "{id}");
            assert_eq!(body["system"][0]["text"], "You are a coding agent.", "{id}");
            assert_eq!(body["tools"][0]["name"], "edit", "{id}");
            assert_eq!(body["thinking"]["type"], "adaptive", "{id}");
            assert_eq!(body["output_config"]["effort"], "high", "{id}");
            assert_eq!(output.calls.len(), 1, "{id}");
            assert_eq!(output.calls[0].name, "edit");
            assert_eq!(output.calls[0].input, json!({"path":"src/main.rs"}));
            assert_eq!(output.usage.input_tokens, 46, "{id}");
            assert_eq!(output.usage.output_tokens, 9, "{id}");
            assert!(
                output.messages[0].content.to_string().contains("sig-1"),
                "{id}"
            );
        }
    }
    #[test]
    fn configured_connections_keep_their_ids_over_built_ins() {
        let directory = tempfile::tempdir().unwrap();
        let mut custom = connection();
        custom["id"] = json!("kimi");
        let path = directory.path().join("providers.json");
        std::fs::write(&path, json!({"connections":[custom]}).to_string()).unwrap();
        let models = ApiModels::load(&path, keyed(directory.path())).unwrap();
        let (_, credential) = models.credential("api/kimi/org/model").unwrap();
        assert_eq!(
            credential.route.endpoint,
            "https://example.invalid/v1/responses"
        );
        assert!(models.credential("api/kimi/kimi-k3.1-pro").is_err());
        assert!(models.credential("api/glm/glm-5.3").is_ok());
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
