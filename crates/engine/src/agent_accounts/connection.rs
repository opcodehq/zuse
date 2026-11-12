//! How Claude Code reaches Anthropic: the CLI's own login, or a local proxy.
//!
//! Some people run a proxy that serves the Anthropic Messages API on
//! loopback and pools several Claude subscriptions behind it (CLIProxyAPI
//! is the common one: `http://127.0.0.1:8317`, access keys from its
//! `api-keys` config). Zuse does not pool anything itself: it remembers one
//! proxy endpoint and, while the connection is set to it, launches Claude
//! with `ANTHROPIC_BASE_URL` and `ANTHROPIC_AUTH_TOKEN`.
//!
//! - **Choose**: `SetClaudeConnection {mode: "proxy"}` without an address
//!   looks for a running CLIProxyAPI (its config file's port and keys, else
//!   the default port); with one, it checks `GET /v1/models` before saving.
//!   Switching back to the account keeps the proxy saved for next time.
//! - **Launch**: the engine stamps every Claude run with the connection
//!   ([`zuse_providers::claude::CONNECTION_OPTION`]); a run stamped
//!   differently from its live process gets a new one. With `fallback` on,
//!   a proxy that doesn't accept a connection at launch is skipped and the
//!   run uses the account instead.
//! - **Store**: `{data_dir}/agent-accounts/claude-connection.json`,
//!   owner-only. The key never leaves the engine.

use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use zuse_protocol::{ClaudeConnection, ClaudeConnectionMode, ProxyHealth, RunRequest};

use super::{AgentAccounts, lock, write_file_atomic};
use crate::EngineError;
use crate::repos::home_dir;

/// CLIProxyAPI's default listen address.
const DEFAULT_PROXY_URL: &str = "http://127.0.0.1:8317";
/// Stamped on runs that use the CLI's own login.
const DIRECT: &str = "direct";
/// What a key-less proxy receives, so the CLI never forwards its own OAuth
/// token to a third-party endpoint.
const NO_KEY: &str = "zuse-no-key";
/// How long a launch waits to learn whether the proxy is up.
const LAUNCH_PROBE: Duration = Duration::from_millis(400);

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ConnectionStore {
    #[serde(default)]
    mode: ClaudeConnectionMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    proxy: Option<Endpoint>,
    #[serde(default = "yes")]
    fallback: bool,
}

fn yes() -> bool {
    true
}

impl Default for ConnectionStore {
    fn default() -> Self {
        Self {
            mode: ClaudeConnectionMode::Account,
            proxy: None,
            fallback: true,
        }
    }
}

impl ConnectionStore {
    pub(super) fn load(file: &std::path::Path) -> Self {
        let mut store: Self = std::fs::read_to_string(file)
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default();
        if store.proxy.is_none() {
            store.mode = ClaudeConnectionMode::Account;
        }
        store
    }

    /// The proxy in force, if the connection is set to it.
    fn active_proxy(&self) -> Option<&Endpoint> {
        (self.mode == ClaudeConnectionMode::Proxy)
            .then_some(self.proxy.as_ref())
            .flatten()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Endpoint {
    base_url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    api_key: Option<String>,
}

impl Endpoint {
    /// Changes whenever the address or key does, so a live process launched
    /// against the old one is replaced. The key itself is hashed, not stamped.
    fn stamp(&self) -> String {
        let digest = Sha256::digest(
            format!(
                "{}\n{}",
                self.base_url,
                self.api_key.as_deref().unwrap_or("")
            )
            .as_bytes(),
        );
        let short: String = digest.iter().take(6).map(|b| format!("{b:02x}")).collect();
        format!("proxy-{short}")
    }

    fn environment(&self) -> zuse_providers::credentials::ChildEnvironment {
        let key = self.api_key.clone().unwrap_or_else(|| NO_KEY.into());
        zuse_providers::credentials::ChildEnvironment::new(
            "ANTHROPIC_BASE_URL",
            Zeroizing::new(self.base_url.clone()),
        )
        .and("ANTHROPIC_AUTH_TOKEN", Zeroizing::new(key))
    }

    fn host_port(&self) -> Option<(String, u16)> {
        let url = reqwest::Url::parse(&self.base_url).ok()?;
        Some((url.host_str()?.to_owned(), url.port_or_known_default()?))
    }
}

/// What `SetClaudeConnection` asks for.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetClaudeConnection {
    pub mode: ClaudeConnectionMode,
    #[serde(default)]
    pub base_url: Option<String>,
    /// `Some("")` clears a saved key; `None` keeps it for the same address.
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub fallback: Option<bool>,
}

impl AgentAccounts {
    fn connection_file(&self) -> PathBuf {
        self.inner.config.root_dir().join("claude-connection.json")
    }

    fn save_connection(&self, store: ConnectionStore) -> Result<(), EngineError> {
        self.inner.config.private_root()?;
        let json = serde_json::to_string_pretty(&store)
            .map_err(|e| EngineError::Other(format!("serialize connection: {e}")))?;
        write_file_atomic(&self.connection_file(), json.as_bytes(), true)?;
        *lock(&self.inner.connection) = store;
        Ok(())
    }

    pub fn claude_connection_mode(&self) -> ClaudeConnectionMode {
        let store = lock(&self.inner.connection);
        if store.active_proxy().is_some() {
            ClaudeConnectionMode::Proxy
        } else {
            ClaudeConnectionMode::Account
        }
    }

    /// The stamp a new Claude run carries: `direct`, or the proxy's.
    pub fn claude_connection(&self) -> String {
        lock(&self.inner.connection)
            .active_proxy()
            .map_or_else(|| DIRECT.into(), Endpoint::stamp)
    }

    /// The proxy a run should launch against. A run stamped `direct` stays
    /// on the account; anything else (a proxy stamp, or no stamp at all for
    /// titles and model discovery) follows the current setting.
    fn proxy_for(&self, request: Option<&RunRequest>) -> Option<Endpoint> {
        let stamped = request
            .and_then(|r| {
                r.model_options
                    .get(zuse_providers::claude::CONNECTION_OPTION)
            })
            .and_then(|v| v.as_str());
        if stamped == Some(DIRECT) {
            return None;
        }
        lock(&self.inner.connection).active_proxy().cloned()
    }

    /// The environment for one Claude process, without probing the proxy.
    pub fn claude_environment(
        &self,
        request: Option<&RunRequest>,
    ) -> Option<zuse_providers::credentials::ChildEnvironment> {
        self.proxy_for(request).map(|proxy| proxy.environment())
    }

    /// [`Self::claude_environment`], falling back to the account when the
    /// proxy refuses a connection and fallback is on.
    pub async fn claude_launch_environment(
        &self,
        request: Option<&RunRequest>,
    ) -> Option<zuse_providers::credentials::ChildEnvironment> {
        let proxy = self.proxy_for(request)?;
        let fallback = lock(&self.inner.connection).fallback;
        if fallback && !accepts_connections(&proxy).await {
            tracing::warn!(
                base_url = %proxy.base_url,
                "Claude proxy is not answering; this run uses the account instead"
            );
            return None;
        }
        Some(proxy.environment())
    }

    /// The current setting, with the proxy's health checked when it is the
    /// connection in use.
    pub async fn get_claude_connection(&self) -> ClaudeConnection {
        let store = lock(&self.inner.connection).clone();
        let mut view = view(&store);
        if let Some(proxy) = store.active_proxy() {
            let (health, message) = match self.check_endpoint(proxy).await {
                Ok(()) => (ProxyHealth::Connected, None),
                Err(Check::Unreachable(m)) => (ProxyHealth::Unreachable, Some(m)),
                Err(Check::Rejected(m)) => (ProxyHealth::Rejected, Some(m)),
            };
            view.health = Some(health);
            view.message = message;
        }
        view
    }

    pub async fn set_claude_connection(
        &self,
        request: SetClaudeConnection,
    ) -> Result<ClaudeConnection, EngineError> {
        let mut store = lock(&self.inner.connection).clone();
        if let Some(fallback) = request.fallback {
            store.fallback = fallback;
        }
        if request.mode == ClaudeConnectionMode::Account {
            store.mode = ClaudeConnectionMode::Account;
            self.save_connection(store)?;
            return Ok(self.get_claude_connection().await);
        }
        let endpoint = match request.base_url.as_deref().map(str::trim) {
            Some(address) if !address.is_empty() => {
                let mut endpoint = parse_endpoint(address)?;
                match request.api_key.as_deref().map(str::trim) {
                    Some("") => endpoint.api_key = None,
                    Some(key) => endpoint.api_key = Some(key.to_owned()),
                    // Same address, no new key: keep the saved one.
                    None if endpoint.api_key.is_none() => {
                        endpoint.api_key = store
                            .proxy
                            .as_ref()
                            .filter(|p| p.base_url == endpoint.base_url)
                            .and_then(|p| p.api_key.clone());
                    }
                    None => {}
                }
                self.check_endpoint(&endpoint)
                    .await
                    .map_err(|c| EngineError::Other(c.message()))?;
                endpoint
            }
            _ => match store.proxy.clone() {
                Some(saved) => saved,
                None => self.detect_local_proxy().await.ok_or_else(|| {
                    EngineError::Other(
                        "No CLI proxy found on this computer. Enter its address and key.".into(),
                    )
                })?,
            },
        };
        store.proxy = Some(endpoint);
        store.mode = ClaudeConnectionMode::Proxy;
        self.save_connection(store)?;
        Ok(self.get_claude_connection().await)
    }

    /// Forget the saved proxy and go back to the account.
    pub fn forget_claude_proxy(&self) -> Result<ClaudeConnection, EngineError> {
        let mut store = lock(&self.inner.connection).clone();
        store.proxy = None;
        store.mode = ClaudeConnectionMode::Account;
        self.save_connection(store.clone())?;
        Ok(view(&store))
    }

    /// A CLIProxyAPI on this machine: its port and access keys from the
    /// first config file found, else the default port with no key. Only an
    /// endpoint that answers `/v1/models` counts.
    async fn detect_local_proxy(&self) -> Option<Endpoint> {
        let config = cliproxy_config_paths()
            .into_iter()
            .find_map(|path| std::fs::read_to_string(path).ok())
            .map(|raw| parse_cliproxy_config(&raw));
        let base_url = config.as_ref().and_then(|c| c.port).map_or_else(
            || DEFAULT_PROXY_URL.into(),
            |port| format!("http://127.0.0.1:{port}"),
        );
        let mut keys: Vec<Option<String>> = config
            .map(|c| c.api_keys.into_iter().map(Some).collect())
            .unwrap_or_default();
        keys.push(None);
        for api_key in keys {
            let endpoint = Endpoint {
                base_url: base_url.clone(),
                api_key,
            };
            if self.check_endpoint(&endpoint).await.is_ok() {
                return Some(endpoint);
            }
        }
        None
    }

    /// `GET {base}/v1/models` with the key both ways proxies accept it.
    async fn check_endpoint(&self, endpoint: &Endpoint) -> Result<(), Check> {
        let mut request = self
            .inner
            .http
            .get(format!("{}/v1/models", endpoint.base_url))
            .header("anthropic-version", "2023-06-01");
        if let Some(key) = &endpoint.api_key {
            request = request.header("x-api-key", key).bearer_auth(key);
        }
        let response = request.send().await.map_err(|error| {
            Check::Unreachable(if error.is_connect() || error.is_timeout() {
                format!(
                    "Nothing answered at {} — is the proxy running?",
                    endpoint.base_url
                )
            } else {
                format!("Couldn't reach {}: {error}", endpoint.base_url)
            })
        })?;
        match response.status().as_u16() {
            200..=299 => Ok(()),
            401 | 403 => Err(Check::Rejected(match endpoint.api_key {
                Some(_) => "The proxy rejected that key.".into(),
                None => "The proxy wants an access key.".into(),
            })),
            404 => Err(Check::Rejected(format!(
                "{} answered, but has no Anthropic /v1/models endpoint.",
                endpoint.base_url
            ))),
            status => Err(Check::Rejected(format!(
                "The proxy answered with HTTP {status}."
            ))),
        }
    }
}

#[derive(Debug)]
enum Check {
    Unreachable(String),
    Rejected(String),
}

impl Check {
    fn message(self) -> String {
        match self {
            Check::Unreachable(m) | Check::Rejected(m) => m,
        }
    }
}

fn view(store: &ConnectionStore) -> ClaudeConnection {
    ClaudeConnection {
        mode: store.mode,
        base_url: store.proxy.as_ref().map(|p| p.base_url.clone()),
        has_key: store.proxy.as_ref().is_some_and(|p| p.api_key.is_some()),
        fallback: store.fallback,
        health: None,
        message: None,
    }
}

async fn accepts_connections(proxy: &Endpoint) -> bool {
    let Some((host, port)) = proxy.host_port() else {
        return false;
    };
    matches!(
        tokio::time::timeout(
            LAUNCH_PROBE,
            tokio::net::TcpStream::connect((host.as_str(), port))
        )
        .await,
        Ok(Ok(_))
    )
}

/// Hands each Claude process the environment of its connection.
pub struct ClaudeConnections(pub AgentAccounts);

#[async_trait::async_trait]
impl zuse_providers::credentials::CredentialResolver for ClaudeConnections {
    /// Unused by the Claude harness, which asks [`Self::environment_for`];
    /// a caller without a run gets the current connection.
    async fn environment(
        &self,
    ) -> Result<zuse_providers::credentials::ChildEnvironment, zuse_providers::HarnessError> {
        self.0.claude_launch_environment(None).await.ok_or_else(|| {
            zuse_providers::HarnessError::Protocol("claude-connection-is-direct".into())
        })
    }
    async fn environment_for(
        &self,
        request: Option<&RunRequest>,
    ) -> Result<Option<zuse_providers::credentials::ChildEnvironment>, zuse_providers::HarnessError>
    {
        Ok(self.0.claude_launch_environment(request).await)
    }
    /// A switch restarts Claude through the run stamp, and a fallback launch
    /// on the account must not be treated as a changed credential.
    fn revalidates(&self) -> bool {
        false
    }
}

/// `URL [key]` in any order, a bare key (default address), or a pasted env
/// snippet (`export ANTHROPIC_BASE_URL="…"`, `ANTHROPIC_AUTH_TOKEN=…`).
pub(super) fn parse_endpoint(input: &str) -> Result<Endpoint, EngineError> {
    let mut base_url = None;
    let mut api_key = None;
    for raw in input.split_whitespace() {
        if raw == "export" {
            continue;
        }
        let (name, value) = match raw.split_once('=') {
            Some((name, value)) if name.chars().all(|c| c.is_ascii_uppercase() || c == '_') => {
                (Some(name), value)
            }
            _ => (None, raw),
        };
        let value = value.trim_matches(|c| c == '"' || c == '\'');
        if value.is_empty() {
            continue;
        }
        let looks_like_url = value.starts_with("http://")
            || value.starts_with("https://")
            || value.starts_with("localhost:")
            || value.starts_with("127.0.0.1:");
        match name {
            Some("ANTHROPIC_BASE_URL") => base_url = Some(value.to_owned()),
            Some("ANTHROPIC_AUTH_TOKEN" | "ANTHROPIC_API_KEY") => api_key = Some(value.to_owned()),
            Some(_) => {}
            None if looks_like_url => base_url = Some(value.to_owned()),
            None => api_key = Some(value.to_owned()),
        }
    }
    let mut base_url = base_url.unwrap_or_else(|| DEFAULT_PROXY_URL.into());
    if !base_url.contains("://") {
        base_url = format!("http://{base_url}");
    }
    let base_url = base_url
        .trim_end_matches('/')
        .trim_end_matches("/v1")
        .trim_end_matches('/')
        .to_owned();
    let parsed = reqwest::Url::parse(&base_url)
        .map_err(|_| EngineError::Other(format!("“{base_url}” isn't a valid address.")))?;
    if parsed.host_str().is_none() {
        return Err(EngineError::Other(format!("“{base_url}” has no host.")));
    }
    Ok(Endpoint { base_url, api_key })
}

/// Where CLIProxyAPI's config usually lives (its own dir, Homebrew, the XDG
/// config dir, or next to a checkout in the home dir).
fn cliproxy_config_paths() -> Vec<PathBuf> {
    let home = home_dir();
    vec![
        home.join(".cli-proxy-api").join("config.yaml"),
        home.join(".config").join("cliproxyapi").join("config.yaml"),
        home.join(".config")
            .join("cli-proxy-api")
            .join("config.yaml"),
        home.join("cliproxyapi").join("config.yaml"),
        home.join("CLIProxyAPI").join("config.yaml"),
        PathBuf::from("/opt/homebrew/etc/cliproxyapi.conf"),
        PathBuf::from("/usr/local/etc/cliproxyapi.conf"),
    ]
}

#[derive(Debug, Default, PartialEq, Eq)]
struct CliProxyConfig {
    port: Option<u16>,
    api_keys: Vec<String>,
}

/// The two top-level fields we need from CLIProxyAPI's YAML: `port:` and
/// the `api-keys:` list. A line scan — the file is flat at those keys, and
/// pulling in a YAML parser for two fields isn't worth it.
fn parse_cliproxy_config(raw: &str) -> CliProxyConfig {
    let mut config = CliProxyConfig::default();
    let mut in_keys = false;
    for line in raw.lines() {
        let without_comment = line.split(" #").next().unwrap_or(line);
        if without_comment.trim().is_empty() || without_comment.trim_start().starts_with('#') {
            continue;
        }
        let top_level = !line.starts_with([' ', '\t', '-']);
        if top_level {
            in_keys = false;
            let unquote = |v: &str| v.trim().trim_matches(|c| c == '"' || c == '\'').to_owned();
            if let Some(value) = without_comment.strip_prefix("port:") {
                config.port = unquote(value).parse().ok();
            } else if let Some(value) = without_comment.strip_prefix("api-keys:") {
                let value = value.trim();
                if let Some(inline) = value.strip_prefix('[').and_then(|v| v.strip_suffix(']')) {
                    config
                        .api_keys
                        .extend(inline.split(',').map(unquote).filter(|k| !k.is_empty()));
                } else {
                    in_keys = true;
                }
            }
        } else if in_keys && let Some(item) = without_comment.trim_start().strip_prefix('-') {
            let key = item.trim().trim_matches(|c| c == '"' || c == '\'');
            if !key.is_empty() {
                config.api_keys.push(key.to_owned());
            }
        }
    }
    config
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_accounts::AgentAccountsConfig;

    #[test]
    fn parses_pasted_endpoints() {
        let parse = |s: &str| parse_endpoint(s).unwrap();
        assert_eq!(
            parse("http://127.0.0.1:8317/ sk-1"),
            Endpoint {
                base_url: "http://127.0.0.1:8317".into(),
                api_key: Some("sk-1".into())
            }
        );
        assert_eq!(
            parse("localhost:9000/v1"),
            Endpoint {
                base_url: "http://localhost:9000".into(),
                api_key: None
            }
        );
        assert_eq!(
            parse(
                "export ANTHROPIC_BASE_URL=\"https://proxy.lan:8443\"\nexport ANTHROPIC_AUTH_TOKEN='k3'"
            ),
            Endpoint {
                base_url: "https://proxy.lan:8443".into(),
                api_key: Some("k3".into())
            }
        );
        assert!(parse_endpoint("http://").is_err());
    }

    #[test]
    fn reads_cliproxy_port_and_keys() {
        let raw = "# comment\nport: 8400\nauth-dir: \"~/.cli-proxy-api\"\napi-keys:\n  - \"key-a\"\n  - key-b # trailing\nrouting:\n  strategy: round-robin\n  - not-a-key\n";
        assert_eq!(
            parse_cliproxy_config(raw),
            CliProxyConfig {
                port: Some(8400),
                api_keys: vec!["key-a".into(), "key-b".into()]
            }
        );
        assert_eq!(
            parse_cliproxy_config("api-keys: [\"x\", 'y']\n").api_keys,
            vec!["x".to_string(), "y".to_string()]
        );
    }

    #[tokio::test]
    async fn switches_between_account_and_proxy() {
        use axum::{Router, http::HeaderMap, http::StatusCode, routing::get};
        let app = Router::new().route(
            "/v1/models",
            get(|headers: HeaderMap| async move {
                if headers.get("x-api-key").is_some_and(|v| v == "good") {
                    (StatusCode::OK, "{\"data\":[]}")
                } else {
                    (StatusCode::UNAUTHORIZED, "{}")
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let dir = tempfile::tempdir().unwrap();
        let config = AgentAccountsConfig::isolated(dir.path());
        let accounts = AgentAccounts::new(config.clone());
        assert_eq!(accounts.claude_connection(), DIRECT);
        let address = format!("http://127.0.0.1:{port}");
        let set = |key: &str| SetClaudeConnection {
            mode: ClaudeConnectionMode::Proxy,
            base_url: Some(address.clone()),
            api_key: Some(key.into()),
            fallback: None,
        };
        let refused = accounts
            .set_claude_connection(set("bad"))
            .await
            .unwrap_err();
        assert!(
            refused.to_string().contains("rejected that key"),
            "{refused}"
        );
        assert_eq!(
            accounts.claude_connection(),
            DIRECT,
            "a refused proxy is not saved"
        );

        let view = accounts.set_claude_connection(set("good")).await.unwrap();
        assert_eq!(view.mode, ClaudeConnectionMode::Proxy);
        assert_eq!(view.health, Some(ProxyHealth::Connected));
        assert!(view.has_key && view.fallback);
        let stamp = accounts.claude_connection();
        assert!(stamp.starts_with("proxy-"));
        let env = accounts.claude_launch_environment(None).await.unwrap();
        assert_eq!(env.get("ANTHROPIC_BASE_URL"), Some(address.as_str()));
        assert_eq!(env.get("ANTHROPIC_AUTH_TOKEN"), Some("good"));

        // A run keeps the account it was stamped with.
        let mut request: RunRequest = serde_json::from_value(
            serde_json::json!({"prompt":"hi","cwd":"/tmp","sandbox":"read-only"}),
        )
        .unwrap();
        request.model_options.insert(
            zuse_providers::claude::CONNECTION_OPTION.into(),
            DIRECT.into(),
        );
        assert!(accounts.claude_environment(Some(&request)).is_none());

        // Survives a restart.
        assert_eq!(
            AgentAccounts::new(config.clone()).claude_connection(),
            stamp
        );

        // A dead proxy: the run falls back to the account; health says why.
        server.abort();
        let _ = server.await;
        assert!(accounts.claude_launch_environment(None).await.is_none());
        let view = accounts.get_claude_connection().await;
        assert_eq!(view.health, Some(ProxyHealth::Unreachable));

        // Back to the account keeps the proxy for next time.
        let view = accounts
            .set_claude_connection(SetClaudeConnection::default())
            .await
            .unwrap();
        assert_eq!(view.mode, ClaudeConnectionMode::Account);
        assert_eq!(view.base_url.as_deref(), Some(address.as_str()));
        assert_eq!(accounts.claude_connection(), DIRECT);
        let view = accounts.forget_claude_proxy().unwrap();
        assert!(view.base_url.is_none());
    }
}
