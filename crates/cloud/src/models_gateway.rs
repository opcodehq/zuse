//! Zuse-hosted models: an OpenAI-compatible chat-completions proxy. The upstream
//! key never leaves the server; callers are WorkOS accounts gated by an allowlist
//! and their prepaid credit balance, which every request debits.
use crate::{access::Access, billing::BillingControl, error::ApiError, store::Store};
use axum::{
    body::{Body, Bytes},
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, OnceLock},
    time::Duration,
};

const MAX_IN_FLIGHT: usize = 8;

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostedModel {
    pub id: String,
    pub label: String,
    /// Wire ID at the upstream; defaults to `id`.
    #[serde(default, skip_serializing)]
    pub upstream: Option<String>,
    pub context_window_tokens: u64,
    #[serde(default = "output_limit")]
    pub max_output_tokens: u32,
    #[serde(default)]
    pub tools: bool,
    #[serde(default)]
    pub images: bool,
    #[serde(default)]
    pub reasoning: bool,
    /// USD micros per million tokens, already including markup.
    #[serde(default)]
    pub input_micros_per_million: u64,
    #[serde(default)]
    pub output_micros_per_million: u64,
}
fn output_limit() -> u32 {
    16384
}

/// Deliberately not `Debug`: it holds the upstream key.
pub struct ModelsGateway {
    endpoint: String,
    key: String,
    owners: Option<Vec<String>>,
    min_balance_cents: i64,
    models: Vec<HostedModel>,
    http: reqwest::Client,
    in_flight: Mutex<HashMap<String, usize>>,
}

impl ModelsGateway {
    /// `None` when the experiment is not configured; routes then answer 404.
    pub fn from_env() -> anyhow::Result<Option<Self>> {
        let (Ok(base), Ok(key)) = (
            std::env::var("MODELS_UPSTREAM_BASE_URL"),
            std::env::var("MODELS_UPSTREAM_API_KEY"),
        ) else {
            return Ok(None);
        };
        if base.trim().is_empty() || key.trim().is_empty() {
            return Ok(None);
        }
        let url = reqwest::Url::parse(base.trim())?;
        anyhow::ensure!(
            url.scheme() == "https",
            "MODELS_UPSTREAM_BASE_URL must be https"
        );
        let models: Vec<HostedModel> = serde_json::from_str(
            &std::env::var("MODELS_CATALOG")
                .map_err(|_| anyhow::anyhow!("MODELS_CATALOG is required"))?,
        )?;
        anyhow::ensure!(
            !models.is_empty()
                && models.len() <= 512
                && models.iter().all(|m| !m.id.is_empty()
                    && !m.id.chars().any(char::is_whitespace)
                    && m.context_window_tokens > 0
                    && m.max_output_tokens > 0),
            "MODELS_CATALOG is invalid"
        );
        // Unset means nobody; `*` opens the experiment to every signed-in account.
        let owners = std::env::var("MODELS_ALLOWED_OWNERS").unwrap_or_default();
        let owners = (owners.trim() != "*").then(|| {
            owners
                .split(',')
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .map(str::to_owned)
                .collect()
        });
        let min_balance_cents = std::env::var("MODELS_MIN_BALANCE_CENTS")
            .ok()
            .map(|v| v.parse())
            .transpose()?
            .unwrap_or(100_i64)
            .max(1);
        Ok(Some(Self {
            endpoint: format!("{}/chat/completions", base.trim().trim_end_matches('/')),
            key: key.trim().into(),
            owners,
            min_balance_cents,
            models,
            http: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .connect_timeout(Duration::from_secs(15))
                .build()?,
            in_flight: Mutex::new(HashMap::new()),
        }))
    }
    pub fn shared() -> Option<&'static Arc<Self>> {
        static GATEWAY: OnceLock<Option<Arc<ModelsGateway>>> = OnceLock::new();
        GATEWAY
            .get_or_init(|| match Self::from_env() {
                Ok(gateway) => gateway.map(Arc::new),
                Err(error) => {
                    tracing::error!(%error, "Hosted models disabled: invalid configuration");
                    None
                }
            })
            .as_ref()
    }
    fn admit(&self, access: &Access) -> Result<(), ApiError> {
        match &self.owners {
            Some(owners) if !owners.contains(&access.owner_id) => {
                Err(ApiError::forbidden("hosted_models_not_enabled"))
            }
            _ => Ok(()),
        }
    }
    pub fn catalog(&self, access: &Access) -> Result<Value, ApiError> {
        self.admit(access)?;
        Ok(json!({ "models": self.models }))
    }
    /// Spendable micros, once the account clears the minimum balance.
    async fn funded(&self, billing: &BillingControl, owner: &str) -> Result<i64, ApiError> {
        let spendable = billing.hosted_spendable_micros(owner).await?;
        if spendable < self.min_balance_cents.saturating_mul(10_000) {
            return Err(ApiError::new(
                StatusCode::PAYMENT_REQUIRED,
                "hosted_models_insufficient_credits",
            ));
        }
        Ok(spendable)
    }
    pub async fn complete(
        self: &Arc<Self>,
        store: &Store,
        access: &Access,
        body: Bytes,
    ) -> Result<Response, ApiError> {
        self.admit(access)?;
        let mut body: Value =
            serde_json::from_slice(&body).map_err(|_| ApiError::bad_request("invalid_request"))?;
        let model = body["model"]
            .as_str()
            .and_then(|id| self.models.iter().find(|m| m.id == id))
            .cloned()
            .ok_or_else(|| ApiError::bad_request("unknown_hosted_model"))?;
        let billing = store
            .billing
            .clone()
            .ok_or_else(|| ApiError::unavailable("hosted_models_billing_unavailable"))?;
        let spendable = self.funded(&billing, &access.owner_id).await?;
        let slot = Slot::take(self.clone(), &access.owner_id)?;
        let request = uuid::Uuid::new_v4().to_string();
        // Prompt bytes over four approximate input tokens if the upstream never reports usage.
        let estimated_input = body.to_string().len().div_ceil(4) as u64;
        let object = body
            .as_object_mut()
            .ok_or_else(|| ApiError::bad_request("invalid_request"))?;
        object.insert(
            "model".into(),
            json!(model.upstream.as_deref().unwrap_or(&model.id)),
        );
        // Usage must come back so every request can be metered.
        object.insert("stream".into(), json!(true));
        object.insert("stream_options".into(), json!({"include_usage": true}));
        for limit in ["max_tokens", "max_completion_tokens"] {
            if let Some(n) = object.get(limit).and_then(Value::as_u64) {
                object.insert(limit.into(), json!(n.min(model.max_output_tokens as u64)));
            }
        }
        let output_limit = ["max_tokens", "max_completion_tokens"]
            .iter()
            .filter_map(|limit| object.get(*limit).and_then(Value::as_u64))
            .min()
            .unwrap_or(model.max_output_tokens as u64);
        object.remove("user");
        let estimate = price(estimated_input, model.input_micros_per_million);
        // Holds cover the longest allowed reply, never more than the account can spend.
        let hold = estimate
            .saturating_add(price(output_limit, model.output_micros_per_million))
            .min(spendable)
            .max(1);
        billing
            .open_model_request(&access.owner_id, &request, &model.id, estimate, hold, now())
            .await?;
        let mut meter = Meter {
            billing,
            request,
            model,
            pending: Vec::new(),
            usage: None,
            estimated_input,
            streamed_chars: 0,
            _slot: slot,
        };
        let response = self
            .http
            .post(&self.endpoint)
            .bearer_auth(&self.key)
            .header(header::ACCEPT, "text/event-stream")
            .json(&body)
            .send()
            .await;
        let response = match response {
            Ok(response) => response,
            Err(_) => {
                meter.void();
                return Err(ApiError::new(
                    StatusCode::BAD_GATEWAY,
                    "hosted_model_unavailable",
                ));
            }
        };
        let status = response.status();
        if !status.is_success() {
            meter.void();
            // Upstream bodies may name the provider account; never pass them through.
            return Err(ApiError::new(
                if status == StatusCode::TOO_MANY_REQUESTS {
                    StatusCode::TOO_MANY_REQUESTS
                } else {
                    StatusCode::BAD_GATEWAY
                },
                "hosted_model_failed",
            ));
        }
        let stream = response.bytes_stream().map(move |chunk| {
            let chunk = chunk.map_err(std::io::Error::other)?;
            meter.observe(&chunk);
            Ok::<_, std::io::Error>(chunk)
        });
        let mut response = Body::from_stream(stream).into_response();
        response.headers_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("text/event-stream"),
        );
        response
            .headers_mut()
            .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
        Ok(response)
    }
}

/// Per-owner concurrency cap; released when the response stream drops.
struct Slot {
    gateway: Arc<ModelsGateway>,
    owner: String,
}
impl Slot {
    fn take(gateway: Arc<ModelsGateway>, owner: &str) -> Result<Self, ApiError> {
        {
            let mut counts = gateway.in_flight.lock().unwrap_or_else(|e| e.into_inner());
            let count = counts.entry(owner.into()).or_default();
            if *count >= MAX_IN_FLIGHT {
                return Err(ApiError::new(
                    StatusCode::TOO_MANY_REQUESTS,
                    "hosted_models_busy",
                ));
            }
            *count += 1;
        }
        Ok(Self {
            gateway,
            owner: owner.into(),
        })
    }
}
impl Drop for Slot {
    fn drop(&mut self) {
        let mut counts = self
            .gateway
            .in_flight
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(count) = counts.get_mut(&self.owner) {
            *count -= 1;
            if *count == 0 {
                counts.remove(&self.owner);
            }
        }
    }
}

fn now() -> i64 {
    chrono::Utc::now().timestamp_millis()
}
fn price(tokens: u64, micros_per_million: u64) -> i64 {
    i64::try_from((u128::from(tokens) * u128::from(micros_per_million)).div_ceil(1_000_000))
        .unwrap_or(i64::MAX)
}

/// Reads token usage from the SSE stream as it passes through, and bills it
/// when the stream ends or the client disconnects.
struct Meter {
    billing: Arc<BillingControl>,
    request: String,
    model: HostedModel,
    pending: Vec<u8>,
    /// Upstream-reported `(prompt, completion)` tokens.
    usage: Option<(u64, u64)>,
    estimated_input: u64,
    /// Generated text seen so far, the fallback when usage never arrives.
    streamed_chars: u64,
    _slot: Slot,
}
impl Meter {
    fn observe(&mut self, chunk: &[u8]) {
        self.pending.extend_from_slice(chunk);
        while let Some(end) = self.pending.iter().position(|b| *b == b'\n') {
            let line = self.pending.drain(..=end).collect::<Vec<_>>();
            self.line(&line);
        }
        // An unterminated line longer than this is not a usage frame.
        if self.pending.len() > 1024 * 1024 {
            self.pending.clear();
        }
    }
    fn line(&mut self, line: &[u8]) {
        let Some(data) = line.strip_prefix(b"data:") else {
            return;
        };
        let Ok(value) = serde_json::from_slice::<Value>(data.trim_ascii()) else {
            return;
        };
        for choice in value["choices"].as_array().into_iter().flatten() {
            let delta = &choice["delta"];
            let mut text = [
                &delta["content"],
                &delta["reasoning_content"],
                &delta["reasoning"],
            ]
            .iter()
            .filter_map(|v| v.as_str())
            .map(str::len)
            .sum::<usize>();
            for call in delta["tool_calls"].as_array().into_iter().flatten() {
                text += call["function"]["arguments"].as_str().map_or(0, str::len);
            }
            self.streamed_chars += text as u64;
        }
        let usage = &value["usage"];
        if let (Some(input), Some(output)) = (
            usage["prompt_tokens"].as_u64(),
            usage["completion_tokens"].as_u64(),
        ) {
            self.usage = Some((input, output));
        }
    }
    /// Billed tokens, and whether they are an estimate.
    fn tokens(&self) -> ((u64, u64), bool) {
        match self.usage {
            Some(usage) => (usage, false),
            None => (
                (self.estimated_input, self.streamed_chars.div_ceil(4)),
                true,
            ),
        }
    }
    fn cost_micros(&self) -> i64 {
        let ((input, output), _) = self.tokens();
        price(input, self.model.input_micros_per_million)
            .saturating_add(price(output, self.model.output_micros_per_million))
    }
    /// The upstream refused before generating anything; nothing is billed.
    fn void(&mut self) {
        self.usage = Some((0, 0));
    }
}
impl Drop for Meter {
    fn drop(&mut self) {
        let ((input, output), estimated) = self.tokens();
        let cost = self.cost_micros();
        let billing = self.billing.clone();
        let request = std::mem::take(&mut self.request);
        let model = self.model.id.clone();
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        runtime.spawn(async move {
            let tokens = (input as i64, output as i64);
            if let Err(error) = billing
                .close_model_request(&request, tokens, cost, estimated, now())
                .await
            {
                // The row stays open and bills its input estimate once abandoned.
                tracing::error!(code = error.code, %request, "Hosted model usage not recorded");
            }
            tracing::info!(
                target: "hosted_models_usage",
                %request,
                %model,
                input_tokens = input,
                output_tokens = output,
                cost_micros = cost,
                estimated,
                "Hosted model request"
            );
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gateway() -> Arc<ModelsGateway> {
        Arc::new(ModelsGateway {
            endpoint: String::new(),
            key: String::new(),
            owners: None,
            min_balance_cents: 1,
            models: Vec::new(),
            http: reqwest::Client::new(),
            in_flight: Mutex::new(HashMap::new()),
        })
    }
    /// Lazy pools need a runtime; the dropped meter's write fails harmlessly.
    fn meter() -> Meter {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://localhost/unused")
            .unwrap();
        Meter {
            billing: Arc::new(BillingControl::from_pool(pool, vec![])),
            request: "request".into(),
            model: HostedModel {
                id: "m".into(),
                label: "M".into(),
                upstream: None,
                context_window_tokens: 1,
                max_output_tokens: 1,
                tools: false,
                images: false,
                reasoning: false,
                input_micros_per_million: 400_000,
                output_micros_per_million: 800_000,
            },
            pending: Vec::new(),
            usage: None,
            estimated_input: 2000,
            streamed_chars: 0,
            _slot: Slot::take(gateway(), "owner").unwrap(),
        }
    }

    #[tokio::test]
    async fn usage_is_read_across_split_chunks() {
        let mut meter = meter();
        meter.observe(b"data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\nda");
        meter.observe(b"ta: {\"choices\":[],\"usage\":{\"prompt_tokens\":1000,");
        meter.observe(b"\"completion_tokens\":500}}\n\ndata: [DONE]\n\n");
        assert_eq!(meter.tokens(), ((1000, 500), false));
        assert_eq!(meter.cost_micros(), 800);
    }

    #[tokio::test]
    async fn missing_usage_bills_an_estimate_from_streamed_text() {
        let mut meter = meter();
        meter.observe(
            b"data: {\"choices\":[{\"delta\":{\"reasoning\":\"abcd\",\"content\":\"efgh\"}}]}\n",
        );
        meter.observe(b"data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"function\":{\"arguments\":\"{}\"}}]}}]}\n");
        assert_eq!(meter.tokens(), ((2000, 3), true));
        assert_eq!(meter.cost_micros(), 800 + 3);
    }

    #[tokio::test]
    async fn refused_requests_are_free() {
        let mut meter = meter();
        meter.observe(b"data: {\"choices\":[{\"delta\":{\"content\":\"partial\"}}]}\n");
        meter.void();
        assert_eq!(meter.cost_micros(), 0);
    }

    #[test]
    fn slots_cap_concurrency_per_owner() {
        let gateway = gateway();
        let slots = (0..MAX_IN_FLIGHT)
            .map(|_| Slot::take(gateway.clone(), "a").unwrap())
            .collect::<Vec<_>>();
        assert!(Slot::take(gateway.clone(), "a").is_err());
        assert!(Slot::take(gateway.clone(), "b").is_ok());
        drop(slots);
        assert!(Slot::take(gateway, "a").is_ok());
    }
}
