//! Process boundary to the pinned Executor v2 SDK. No tool calls are retried.
use super::{
    PluginFailure,
    egress::{self, Policy},
};
use crate::{crypto::MasterKey, error::ApiError};
use axum::{
    Json, Router,
    body::Body,
    extract::{DefaultBodyLimit, State},
    http::{HeaderMap, StatusCode},
    response::Response,
    routing::post,
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use futures::StreamExt;
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::ConnectOptions;
use std::{process::Stdio, sync::Arc, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::Child,
    sync::Semaphore,
};

pub struct Executor {
    http: reqwest::Client,
    url: reqwest::Url,
    token: String,
    failed: std::sync::atomic::AtomicBool,
    _child: tokio::sync::Mutex<Child>,
    proxy: tokio::task::JoinHandle<()>,
    stdout: tokio::task::JoinHandle<()>,
}
impl Drop for Executor {
    fn drop(&mut self) {
        self.proxy.abort();
        self.stdout.abort();
    }
}
#[derive(Clone)]
struct Proxy {
    token: String,
    http: reqwest::Client,
    policy: Policy,
    gate: Arc<Semaphore>,
}
#[derive(Deserialize)]
struct Outbound {
    url: String,
    method: String,
    headers: Vec<(String, String)>,
    body: Option<String>,
}
async fn forward(
    State(state): State<Proxy>,
    headers: HeaderMap,
    Json(input): Json<Outbound>,
) -> Result<Response, ApiError> {
    use subtle::ConstantTimeEq;
    let expected = format!("Bearer {}", state.token);
    if !bool::from(
        headers
            .get("authorization")
            .map(|v| v.as_bytes())
            .unwrap_or_default()
            .ct_eq(expected.as_bytes()),
    ) {
        return Err(ApiError::unauthorized("executor_auth_failed"));
    }
    let permit = state
        .gate
        .clone()
        .try_acquire_owned()
        .map_err(|_| ApiError::unavailable("plugins_busy"))?;
    let url = state.policy.parse(&input.url)?;
    let method = match input.method.as_str() {
        "GET" => reqwest::Method::GET,
        "POST" => reqwest::Method::POST,
        "DELETE" => reqwest::Method::DELETE,
        _ => return Err(ApiError::bad_request("executor_method_rejected")),
    };
    let mut request = state.http.request(method, url);
    for (name, value) in input.headers {
        if [
            "host",
            "connection",
            "content-length",
            "transfer-encoding",
            "proxy-authorization",
            "proxy-connection",
        ]
        .contains(&name.to_ascii_lowercase().as_str())
        {
            continue;
        }
        request = request.header(
            reqwest::header::HeaderName::from_bytes(name.as_bytes())
                .map_err(|_| ApiError::bad_request("executor_header_rejected"))?,
            reqwest::header::HeaderValue::from_str(&value)
                .map_err(|_| ApiError::bad_request("executor_header_rejected"))?,
        );
    }
    if let Some(body) = input.body {
        let bytes = STANDARD
            .decode(body)
            .map_err(|_| ApiError::bad_request("executor_body_rejected"))?;
        if bytes.len() > 256 * 1024 {
            return Err(ApiError::bad_request("executor_body_rejected"));
        }
        request = request.body(bytes);
    }
    let upstream = request
        .send()
        .await
        .map_err(|_| ApiError::bad_request("plugin_unreachable"))?;
    if upstream.status().is_redirection() {
        return Err(ApiError::bad_request("plugin_url_rejected"));
    }
    let status = upstream.status();
    let headers = upstream.headers().clone();
    let mut total = 0usize;
    let stream = upstream.bytes_stream().map(move |part| {
        let _keep = &permit;
        part.map_err(std::io::Error::other).and_then(|part| {
            total = total.saturating_add(part.len());
            if total > 8 * 1024 * 1024 {
                Err(std::io::Error::other("Executor response budget exceeded"))
            } else {
                Ok(part)
            }
        })
    });
    let mut response = Response::new(Body::from_stream(stream));
    *response.status_mut() = status;
    for (name, value) in &headers {
        if !["connection", "content-length", "transfer-encoding"].contains(&name.as_str()) {
            response.headers_mut().append(name.clone(), value.clone());
        }
    }
    Ok(response)
}
impl Executor {
    async fn database_url(pool: &sqlx::PgPool) -> Result<String, ApiError> {
        let mut url = pool.connect_options().to_url_lossy();
        let schema: String = sqlx::query_scalar("SELECT current_schema()")
            .fetch_one(pool)
            .await?;
        if !schema
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_')
        {
            return Err(ApiError::unavailable("executor_schema_invalid"));
        }
        let local = matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "[::1]"));
        let pairs: Vec<_> = url
            .query_pairs()
            .filter(|(k, _)| k != "statement-cache-capacity" && k != "options")
            .map(|(k, v)| {
                let v = if local && k == "sslmode" && v == "prefer" {
                    "disable".into()
                } else {
                    v
                };
                (k.into_owned(), v.into_owned())
            })
            .collect();
        url.set_query(None);
        url.query_pairs_mut().extend_pairs(pairs).append_pair(
            "options",
            &format!("-c search_path={schema} -c statement_timeout=15000 -c lock_timeout=5000"),
        );
        Ok(url.into())
    }
    fn command(url: &str, key: &MasterKey, token: &str, proxy: &str) -> tokio::process::Command {
        let script = std::env::var("ZUSE_CLOUD_EXECUTOR_SCRIPT").unwrap_or_else(|_| {
            concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../packages/executor-v2/vendor/host.mjs"
            )
            .into()
        });
        let mut command = tokio::process::Command::new(
            std::env::var("ZUSE_CLOUD_EXECUTOR_BINARY").unwrap_or_else(|_| "bun".into()),
        );
        command
            .arg(script)
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("ZUSE_EXECUTOR_DATABASE_URL", url)
            .env("ZUSE_EXECUTOR_ENCRYPTION_KEY", key.executor_key().as_str())
            .env("ZUSE_EXECUTOR_TOKEN", token)
            .env("ZUSE_EXECUTOR_EGRESS", proxy)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true);
        command
    }
    pub async fn migrate(pool: &sqlx::PgPool, key: &MasterKey) -> Result<(), ApiError> {
        let url = Self::database_url(pool).await?;
        let mut command = Self::command(
            &url,
            key,
            &super::random_token(),
            "http://127.0.0.1:1/fetch",
        );
        let status = command
            .arg("--migrate")
            .status()
            .await
            .map_err(|_| ApiError::unavailable("executor_migration_failed"))?;
        if !status.success() {
            return Err(ApiError::unavailable("executor_migration_failed"));
        }
        Ok(())
    }
    pub async fn start(
        pool: &sqlx::PgPool,
        key: &MasterKey,
        policy: Policy,
    ) -> Result<Arc<Self>, ApiError> {
        let database = Self::database_url(pool).await?;
        let token = super::random_token();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(|_| ApiError::unavailable("executor_start_failed"))?;
        let address = listener
            .local_addr()
            .map_err(|_| ApiError::unavailable("executor_start_failed"))?;
        let state = Proxy {
            token: token.clone(),
            http: policy
                .client()
                .map_err(|_| ApiError::unavailable("executor_start_failed"))?,
            policy,
            gate: Arc::new(Semaphore::new(32)),
        };
        let router = Router::new()
            .route("/fetch", post(forward))
            .layer(DefaultBodyLimit::max(512 * 1024))
            .with_state(state);
        let proxy = tokio::spawn(async move {
            let _ = axum::serve(listener, router).await;
        });
        let mut command = Self::command(&database, key, &token, &format!("http://{address}/fetch"));
        if policy == Policy::Loopback {
            command.env("ZUSE_EXECUTOR_TEST_LOOPBACK", "1");
        }
        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(_) => {
                proxy.abort();
                return Err(ApiError::unavailable("executor_start_failed"));
            }
        };
        let mut lines = BufReader::new(child.stdout.take().expect("piped stdout")).lines();
        let ready = tokio::time::timeout(Duration::from_secs(20), lines.next_line()).await;
        let port = match ready {
            Ok(Ok(Some(line))) => serde_json::from_str::<Value>(&line)
                .ok()
                .and_then(|v| v["port"].as_u64())
                .filter(|p| *p > 0 && *p <= 65535),
            _ => None,
        };
        let Some(port) = port else {
            proxy.abort();
            return Err(ApiError::unavailable("executor_start_failed"));
        };
        let mut output = lines.into_inner();
        let stdout = tokio::spawn(async move {
            let _ = tokio::io::copy(&mut output, &mut tokio::io::sink()).await;
        });
        let instance = Arc::new(Self {
            http: reqwest::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(Duration::from_secs(65))
                .build()
                .map_err(|_| ApiError::unavailable("executor_start_failed"))?,
            url: format!("http://127.0.0.1:{port}/")
                .parse()
                .expect("loopback URL"),
            token,
            failed: std::sync::atomic::AtomicBool::new(false),
            _child: tokio::sync::Mutex::new(child),
            proxy,
            stdout,
        });
        instance.call(json!({"operation":"ready"})).await?;
        Ok(instance)
    }
    pub async fn alive(&self) -> bool {
        !self.failed.load(std::sync::atomic::Ordering::SeqCst)
            && matches!(self._child.lock().await.try_wait(), Ok(None))
    }
    async fn stop(&self) {
        self.failed.store(true, std::sync::atomic::Ordering::SeqCst);
        let _ = self._child.lock().await.start_kill();
    }
    pub async fn call(&self, input: Value) -> Result<Value, ApiError> {
        let response = match self
            .http
            .post(self.url.clone())
            .bearer_auth(&self.token)
            .json(&input)
            .send()
            .await
        {
            Ok(response) => response,
            Err(_) => {
                self.stop().await;
                return Err(ApiError::unavailable("executor_unavailable"));
            }
        };
        let status = response.status();
        if status == StatusCode::SERVICE_UNAVAILABLE {
            return Err(ApiError::unavailable("plugins_busy"));
        }
        let body = match egress::bounded(response, 8 * 1024 * 1024).await {
            Ok(body) => body,
            Err(_) => {
                self.stop().await;
                return Err(ApiError::unavailable("executor_protocol_failed"));
            }
        };
        let output: Value = match serde_json::from_slice(&body) {
            Ok(output) => output,
            Err(_) => {
                self.stop().await;
                return Err(ApiError::unavailable("executor_protocol_failed"));
            }
        };
        if status.is_success() && output["ok"] == true {
            return Ok(output["value"].clone());
        }
        if output["code"] == "needs_auth" {
            return Err(ApiError::bad_request("plugin_needs_auth"));
        }
        let error = match output["code"].as_str() {
            Some("client_registration_unsupported") => PluginFailure::ClientRegistrationUnsupported,
            Some("auth_unsupported") => PluginFailure::AuthUnsupported,
            Some("unreachable") => PluginFailure::Unreachable,
            _ => PluginFailure::OperationFailed,
        };
        Err(error.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn executor_deadline_kills_worker_without_retrying() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = Router::new().route(
            "/",
            post(|| async { std::future::pending::<StatusCode>().await }),
        );
        let proxy = tokio::spawn(async move { axum::serve(listener, server).await.unwrap() });
        // Any long-lived child stands in for a stalled worker.
        let child = tokio::process::Command::new("sleep")
            .arg("60")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let worker = Executor {
            http: reqwest::Client::builder()
                .no_proxy()
                .timeout(Duration::from_millis(50))
                .build()
                .unwrap(),
            url: format!("http://{address}/").parse().unwrap(),
            token: "fixture".into(),
            failed: std::sync::atomic::AtomicBool::new(false),
            _child: tokio::sync::Mutex::new(child),
            proxy,
            stdout: tokio::spawn(std::future::pending()),
        };
        assert!(worker.call(json!({"operation":"call"})).await.is_err());
        assert!(!worker.alive().await);
        let status =
            tokio::time::timeout(Duration::from_secs(2), worker._child.lock().await.wait())
                .await
                .unwrap()
                .unwrap();
        assert!(!status.success());
    }
}
