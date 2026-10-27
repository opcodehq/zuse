//! Native provider protocols. No scheduling, tool execution or credential renewal.
use crate::{Error, ModelInput, ModelOutput, RequestBudget, Usage};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Protocol {
    Responses,
    Messages,
    GenerateContent,
    ChatCompletions,
}
/// Select credentials explicitly; protocol defaults preserve API-key behavior.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Authentication {
    #[default]
    ProtocolDefault,
    Bearer,
}
impl Authentication {
    fn is_protocol_default(&self) -> bool {
        *self == Self::ProtocolDefault
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Capabilities {
    pub images: bool,
    pub tools: bool,
    pub reasoning: bool,
    pub prompt_cache: bool,
}
/// Credentials are owned by the account service; never log this type.
pub struct Credential {
    pub connection_id: String,
    pub access_token: String,
    pub authorization_generation: u64,
    pub route: Route,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Route {
    pub endpoint: String,
    pub protocol: Protocol,
    pub subscription: bool,
    #[serde(default, skip_serializing_if = "Authentication::is_protocol_default")]
    pub authentication: Authentication,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub betas: Vec<String>,
    pub capabilities: Capabilities,
    pub max_output_tokens: u32,
    #[serde(default)]
    pub tool_namespace: Option<String>,
    #[serde(default)]
    pub include_encrypted_reasoning: bool,
}
impl Route {
    pub fn validate_authentication(&self) -> Result<(), Error> {
        if self.betas.len() > 16
            || (!self.betas.is_empty() && self.protocol != Protocol::Messages)
            || self.betas.iter().any(|beta| {
                beta.is_empty()
                    || beta.len() > 128
                    || !beta.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
            })
        {
            return Err(failed("invalid_provider_betas"));
        }
        Ok(())
    }
    pub fn subscription(provider: &str) -> Result<Self, Error> {
        let endpoint = match provider {
            "chatgpt" => "https://api.openai.com/v1/responses",
            "supergrok" => "https://api.x.ai/v1/responses",
            _ => return Err(failed("unsupported_subscription_route")),
        };
        Ok(Self {
            endpoint: endpoint.into(),
            protocol: Protocol::Responses,
            subscription: true,
            authentication: Authentication::ProtocolDefault,
            betas: Vec::new(),
            capabilities: Capabilities {
                images: true,
                tools: true,
                reasoning: true,
                prompt_cache: provider == "chatgpt",
            },
            max_output_tokens: 16384,
            tool_namespace: if provider == "chatgpt" {
                Some("zuse".into())
            } else {
                None
            },
            include_encrypted_reasoning: provider == "chatgpt",
        })
    }
}
#[async_trait]
pub trait ModelTransport: Send + Sync {
    async fn request(
        &self,
        input: ModelInput,
        credential: Credential,
    ) -> Result<ModelOutput, Error>;
    async fn close(&self);
}
#[derive(Clone)]
pub struct NativeTransport {
    client: reqwest::Client,
    stop: CancellationToken,
    admission: Arc<tokio::sync::Semaphore>,
    budget: Option<Arc<dyn RequestBudget>>,
    observer: Arc<dyn Fn(Attempt) + Send + Sync>,
}
pub(super) fn failed(code: &str) -> Error {
    Error::Model {
        code: code.into(),
        status: None,
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Attempt {
    pub request_id: String,
    pub root_id: String,
    pub agent_id: String,
    pub model: String,
    pub connection_id: String,
    pub purpose: crate::RequestPurpose,
    pub retry_index: u32,
    pub billing_route: String,
    pub dispatched: bool,
    pub elapsed_ms: f64,
    pub usage: Option<Usage>,
    pub error_code: Option<String>,
    pub prefix_fingerprint: String,
}
impl NativeTransport {
    pub fn with_budget(mut self, budget: Arc<dyn RequestBudget>) -> Self {
        self.budget = Some(budget);
        self
    }
    pub fn with_observer(mut self, observer: Arc<dyn Fn(Attempt) + Send + Sync>) -> Self {
        self.observer = observer;
        self
    }
    pub fn new() -> Result<Self, Error> {
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(std::time::Duration::from_secs(15))
            .pool_max_idle_per_host(4)
            .build()
            .map_err(|_| failed("transport_initialization"))?;
        Ok(Self {
            client,
            stop: CancellationToken::new(),
            admission: Arc::new(tokio::sync::Semaphore::new(32)),
            budget: None,
            observer: Arc::new(|_| {}),
        })
    }
}
mod request;
mod stream;
use futures::StreamExt;
use serde_json::Value;

#[async_trait]
impl ModelTransport for NativeTransport {
    async fn request(
        &self,
        input: ModelInput,
        credential: Credential,
    ) -> Result<ModelOutput, Error> {
        let mut audit = AttemptGuard::new(&input, &credential, self.observer.clone());
        let result = tokio::select! {biased;_=self.stop.cancelled()=>Err(Error::Interrupted),_=input.cancel.cancelled()=>Err(Error::Interrupted),result=self.send(&input,&credential,&audit)=>result};
        audit.attempt.usage = result.as_ref().ok().map(|output| output.usage.clone());
        audit.attempt.error_code = result.as_ref().err().map(|error| match error {
            Error::Model { code, .. } => code.clone(),
            Error::Interrupted => "interrupted".into(),
            _ => "request_failed".into(),
        });
        result
    }
    async fn close(&self) {
        self.stop.cancel();
        self.admission.close();
    }
}
impl NativeTransport {
    async fn send(
        &self,
        input: &ModelInput,
        credential: &Credential,
        audit: &AttemptGuard,
    ) -> Result<ModelOutput, Error> {
        let _permit = self
            .admission
            .acquire()
            .await
            .map_err(|_| Error::Interrupted)?;
        let url = reqwest::Url::parse(&credential.route.endpoint)
            .map_err(|_| failed("invalid_model_endpoint"))?;
        if url.scheme() != "https"
            && !(url.scheme() == "http"
                && matches!(url.host_str(), Some("127.0.0.1" | "::1" | "localhost")))
        {
            return Err(failed("insecure_model_endpoint"));
        }
        if !url.username().is_empty() || url.password().is_some() {
            return Err(failed("invalid_model_endpoint"));
        }
        credential.route.validate_authentication()?;
        let body = request::body(input, credential)?;
        let body = serde_json::to_vec(&body).map_err(|_| failed("invalid_model_request"))?;
        if body.len() > 96 * 1024 * 1024 {
            return Err(failed("model_input_too_large"));
        }
        let mut request = self
            .client
            .post(url)
            .header("content-type", "application/json")
            .header("accept", "text/event-stream")
            .body(body);
        let mut key = reqwest::header::HeaderValue::from_str(&credential.access_token)
            .map_err(|_| failed("invalid_model_credential"))?;
        key.set_sensitive(true);
        if credential.route.protocol == Protocol::Messages {
            request = request.header("anthropic-version", "2023-06-01");
            if !credential.route.betas.is_empty() {
                request = request.header("anthropic-beta", credential.route.betas.join(","));
            }
        }
        request = match (credential.route.authentication, credential.route.protocol) {
            (Authentication::ProtocolDefault, Protocol::Messages) => {
                request.header("x-api-key", key)
            }
            (Authentication::ProtocolDefault, Protocol::GenerateContent) => {
                request.header("x-goog-api-key", key)
            }
            _ => request.bearer_auth(&credential.access_token),
        };
        if let Some(budget) = &self.budget {
            budget.reserve().await?;
        }
        audit
            .dispatched
            .store(true, std::sync::atomic::Ordering::Release);
        let response = tokio::time::timeout(std::time::Duration::from_secs(120), request.send())
            .await
            .map_err(|_| failed("model_headers_timeout"))?
            .map_err(|_| failed("model_connection_failed"))?;
        let status = response.status();
        if !status.is_success() {
            let mut stream = response.bytes_stream();
            let mut body = Vec::new();
            while let Some(chunk) =
                tokio::time::timeout(std::time::Duration::from_secs(30), stream.next())
                    .await
                    .map_err(|_| failed("model_timeout"))?
            {
                let chunk = chunk.map_err(|_| failed("model_connection_failed"))?;
                if body.len() + chunk.len() > 64 * 1024 {
                    break;
                }
                body.extend_from_slice(&chunk);
            }
            let value: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
            let code = match value["error"]["code"].as_str() {
                Some("subscription_sharing_usage_limit_exceeded") => {
                    "subscription_sharing_usage_limit_exceeded".into()
                }
                Some("subscription_sharing_unavailable") => {
                    "subscription_sharing_unavailable".into()
                }
                _ => format!("http_{}", status.as_u16()),
            };
            return Err(Error::Model {
                code,
                status: Some(status.as_u16()),
            });
        }
        let mut chunks = response.bytes_stream();
        let mut decoder = stream::Sse::default();
        let mut assembly = stream::Assembly::new(audit.started);
        while let Some(chunk) =
            tokio::time::timeout(std::time::Duration::from_secs(120), chunks.next())
                .await
                .map_err(|_| failed("model_stream_timeout"))?
        {
            let chunk = chunk.map_err(|_| failed("stream_interrupted"))?;
            for data in decoder.push(&chunk)? {
                if data == b"[DONE]" {
                    if credential.route.protocol != Protocol::ChatCompletions {
                        return Err(failed("unexpected_stream_terminator"));
                    }
                    // A transport terminator alone cannot establish model completion.
                    assembly.end_chat()?;
                } else {
                    let value =
                        serde_json::from_slice(&data).map_err(|_| failed("invalid_stream_json"))?;
                    let result = assembly.event(input, credential.route.protocol, value);
                    *audit.partial.lock().unwrap_or_else(|e| e.into_inner()) =
                        assembly.partial_usage(credential.route.protocol);
                    result?;
                }
            }
        }
        if !decoder.complete() {
            return Err(failed("stream_incomplete"));
        }
        assembly.finish(input, credential, audit.attempt.request_id.clone())
    }
}
#[cfg(test)]
mod tests;

/// Provider qualification is routing metadata, never part of the wire model ID.
pub fn model_name(model: &str) -> &str {
    if model.starts_with("api/") {
        model.splitn(3, '/').nth(2).unwrap_or(model)
    } else {
        model
            .strip_prefix("chatgpt/")
            .or_else(|| model.strip_prefix("supergrok/"))
            .unwrap_or(model)
    }
}

/// Reports even when an outer cancellation drops the transport future. The
/// observer is diagnostic and cannot turn unwinding into a process abort.
struct AttemptGuard {
    attempt: Attempt,
    started: std::time::Instant,
    dispatched: std::sync::atomic::AtomicBool,
    partial: std::sync::Mutex<Option<Usage>>,
    observer: Arc<dyn Fn(Attempt) + Send + Sync>,
    report: Arc<dyn Fn(Attempt) + Send + Sync>,
}
impl AttemptGuard {
    fn new(
        input: &ModelInput,
        credential: &Credential,
        observer: Arc<dyn Fn(Attempt) + Send + Sync>,
    ) -> Self {
        let started = std::time::Instant::now();
        Self {
            started,
            dispatched: std::sync::atomic::AtomicBool::new(false),
            partial: std::sync::Mutex::new(None),
            observer,
            report: input.report.clone(),
            attempt: Attempt {
                request_id: uuid::Uuid::new_v4().to_string(),
                root_id: input.root_id.clone(),
                agent_id: input.agent_id.clone(),
                model: input.model.clone(),
                connection_id: credential.connection_id.clone(),
                purpose: input.purpose,
                retry_index: input.retry_index,
                billing_route: if credential.route.subscription {
                    "subscription"
                } else {
                    "api"
                }
                .into(),
                dispatched: false,
                elapsed_ms: 0.,
                usage: None,
                error_code: Some("request_abandoned".into()),
                prefix_fingerprint: request::fingerprint(input, credential),
            },
        }
    }
}
impl Drop for AttemptGuard {
    fn drop(&mut self) {
        self.attempt.dispatched = self.dispatched.load(std::sync::atomic::Ordering::Acquire);
        self.attempt.elapsed_ms = self.started.elapsed().as_secs_f64() * 1000.;
        if self.attempt.usage.is_none() {
            self.attempt.usage = self
                .partial
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .take();
        }
        if let Some(usage) = &mut self.attempt.usage {
            usage.connection_id = self.attempt.connection_id.clone();
            usage.request_id = Some(self.attempt.request_id.clone());
            usage.purpose = self.attempt.purpose;
            usage.billing_route = Some(self.attempt.billing_route.clone());
            usage.prefix_fingerprint = Some(self.attempt.prefix_fingerprint.clone());
        }
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            (self.observer)(self.attempt.clone())
        }));
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            (self.report)(self.attempt.clone())
        }));
    }
}
