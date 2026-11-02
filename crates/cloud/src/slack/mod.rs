//! First-party Slack ingress and durable delivery alongside the cloud API.
//! This module owns bot credentials; public settings contain identifiers only.
mod http;
#[cfg(test)]
mod tests;
mod worker;
use crate::{
    access::{Access, AccessKind},
    auth::Principal,
    crypto::MasterKey,
    error::ApiError,
    routes::AppState,
};
use axum::{
    Router,
    extract::DefaultBodyLimit,
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{PgPool, postgres::PgPoolOptions};
use std::{sync::Arc, time::Duration};
use zuse_protocol::slack::ConnectRequest;

pub const MIGRATION: &str = include_str!("../../migrations/0009_slack.sql");
pub struct Config {
    signing_secret: String,
    client_id: String,
    client_secret: String,
    redirect_uri: String,
}
impl Config {
    pub fn from_env() -> anyhow::Result<Option<Self>> {
        if std::env::var("ZUSE_SLACK_ENABLED").as_deref() != Ok("true") {
            return Ok(None);
        }
        let get = |name| {
            std::env::var(name)
                .ok()
                .filter(|s| !s.trim().is_empty())
                .ok_or_else(|| anyhow::anyhow!("{name} is required for Slack"))
        };
        let redirect_uri = get("ZUSE_SLACK_REDIRECT_URI")?;
        let url = reqwest::Url::parse(&redirect_uri)?;
        anyhow::ensure!(
            url.scheme() == "https"
                && url.path() == "/slack/oauth/callback"
                && url.username().is_empty()
                && url.password().is_none()
                && url.query().is_none()
                && url.fragment().is_none(),
            "Slack requires an HTTPS /slack/oauth/callback URL"
        );
        Ok(Some(Self {
            signing_secret: get("SLACK_SIGNING_SECRET")?,
            client_id: get("SLACK_CLIENT_ID")?,
            client_secret: get("SLACK_CLIENT_SECRET")?,
            redirect_uri,
        }))
    }
}
#[derive(Clone)]
pub struct Slack {
    app: AppState,
    pool: PgPool,
    master: Arc<MasterKey>,
    config: Arc<Config>,
    http: reqwest::Client,
    api_origin: String,
    retry_after: Arc<std::sync::atomic::AtomicI64>,
}
impl Slack {
    pub async fn connect(
        app: AppState,
        config: Config,
        database_url: &str,
    ) -> anyhow::Result<Self> {
        anyhow::ensure!(
            app.store.transcripts_enabled(),
            "Slack result delivery requires encrypted transcript storage"
        );
        anyhow::ensure!(
            app.runtime.as_ref().is_some_and(|r| r.creation_enabled()),
            "Slack requires cloud creation and runtime control"
        );
        let pool = PgPoolOptions::new()
            .max_connections(3)
            .acquire_timeout(Duration::from_secs(5))
            .connect(database_url)
            .await?;
        let exists: bool =
            sqlx::query_scalar("SELECT to_regclass('zuse_rust_slack_jobs') IS NOT NULL AND to_regclass('zuse_rust_slack_installs') IS NOT NULL AND to_regclass('zuse_rust_slack_members') IS NOT NULL AND to_regclass('zuse_rust_slack_oauth') IS NOT NULL AND to_regclass('zuse_rust_slack_threads') IS NOT NULL")
                .fetch_one(&pool)
                .await?;
        anyhow::ensure!(exists, "Apply 0009_slack.sql before enabling Slack");
        let master = MasterKey::parse(
            &std::env::var("CLOUD_DATA_ENCRYPTION_KEY")
                .or_else(|_| std::env::var("CLOUD_CREDENTIAL_VAULT_KEY"))?,
        )
        .map_err(|_| anyhow::anyhow!("Invalid Slack encryption key"))?;
        Ok(Self {
            app,
            pool,
            master: Arc::new(master),
            config: Arc::new(config),
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(15))
                .redirect(reqwest::redirect::Policy::none())
                .build()?,
            api_origin: "https://slack.com/api".into(),
            retry_after: Arc::new(std::sync::atomic::AtomicI64::new(0)),
        })
    }
    pub fn router(self) -> Router {
        Router::new()
            .route("/slack/events", post(http::events))
            .route("/slack/oauth/callback", get(http::callback))
            .route("/slack/oauth/start", get(http::start))
            .route("/slack/oauth/authorize", post(http::authorize))
            .route("/v1/cloud/slack", get(http::connections))
            .route("/v1/cloud/slack/connect", post(http::connect))
            .route("/v1/cloud/slack/disconnect", post(http::disconnect))
            .layer(DefaultBodyLimit::max(256 * 1024))
            .with_state(self)
    }
    /// One worker per process; a PostgreSQL advisory lock serializes all replicas.
    pub fn start_worker(&self) -> tokio::task::JoinHandle<()> {
        let service = self.clone();
        tokio::spawn(async move {
            loop {
                if let Err(error) = service.work_once().await {
                    tracing::warn!(code = error.code, "Slack delivery deferred");
                }
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        })
    }
    async fn access(&self, headers: &axum::http::HeaderMap) -> Result<Access, ApiError> {
        crate::access::authorize(
            self.app.identity.as_ref(),
            headers,
            AccessKind::Content,
            self.app.organizations_enabled,
            &self.app.organization_ids,
        )
        .await
    }
    async fn member_access(&self, member: &Member) -> Result<Access, ApiError> {
        let (scope, membership) = match &member.organization_id {
            None if member.account_id == member.actor_id => (crate::access::Scope::Personal, None),
            Some(id)
                if self.app.organizations_enabled && self.app.organization_ids.contains(id) =>
            {
                let m = self.app.identity.membership(&member.actor_id, id).await?;
                if m.status != "active"
                    || m.user_id != member.actor_id
                    || m.organization_id != *id
                    || !matches!(m.role.slug.as_str(), "member" | "admin")
                    || member.account_id != format!("organization:{id}")
                {
                    return Err(ApiError::forbidden("slack_workspace_access_denied"));
                }
                (
                    crate::access::Scope::Organization {
                        organization_id: id.clone(),
                    },
                    Some(m),
                )
            }
            _ => return Err(ApiError::forbidden("slack_workspace_access_denied")),
        };
        Ok(Access {
            actor: Principal {
                account_id: member.actor_id.clone(),
            },
            scope,
            owner_id: member.account_id.clone(),
            membership,
        })
    }
    fn seal<T: Serialize>(&self, context: &str, value: &T) -> Result<String, ApiError> {
        self.master
            .seal(
                context.as_bytes(),
                &serde_json::to_vec(value).map_err(|_| invalid())?,
            )
            .map_err(|_| unavailable())
    }
    fn open<T: for<'de> Deserialize<'de>>(
        &self,
        context: &str,
        value: &str,
    ) -> Result<T, ApiError> {
        let bytes = self
            .master
            .open(context.as_bytes(), value)
            .map_err(|_| unavailable())?;
        serde_json::from_slice(&bytes).map_err(|_| unavailable())
    }
    async fn api(&self, token: &str, method: &str, body: Value) -> Result<Value, ApiError> {
        let url = format!("{}/{method}", self.api_origin);
        let request = if method.starts_with("chat.") {
            self.http.post(url).json(&body)
        } else {
            self.http.get(url).query(&body)
        };
        let response = request
            .bearer_auth(token)
            .send()
            .await
            .map_err(|_| unavailable())?;
        if response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
            let seconds = response
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse::<i64>().ok())
                .unwrap_or(60)
                .clamp(1, 3600);
            self.retry_after
                .fetch_max(now() + seconds * 1000, std::sync::atomic::Ordering::Relaxed);
            return Err(ApiError::unavailable("slack_rate_limited"));
        }
        if !response.status().is_success() {
            return Err(unavailable());
        }
        let bytes = crate::auth::bounded_body(response).await?;
        let value: Value = serde_json::from_slice(&bytes).map_err(|_| unavailable())?;
        if value["ok"] != true {
            return Err(match value["error"].as_str() {
                Some("message_not_found" | "cant_update_message") => {
                    ApiError::bad_request("slack_progress_reply_unavailable")
                }
                Some("missing_scope" | "not_in_channel" | "channel_not_found") => {
                    ApiError::forbidden("slack_channel_access_unavailable")
                }
                _ => unavailable(),
            });
        }
        Ok(value)
    }
}
#[derive(Serialize, Deserialize)]
struct Member {
    account_id: String,
    actor_id: String,
    organization_id: Option<String>,
    settings: ConnectRequest,
    revision: String,
}
#[derive(Clone, Serialize, Deserialize)]
struct Event {
    team: String,
    id: String,
    channel: String,
    user: String,
    ts: String,
    thread: String,
    text: String,
    #[serde(default)]
    unsupported_files: bool,
    #[serde(default)]
    too_large: bool,
    #[serde(default)]
    generation: String,
    #[serde(default)]
    revision: String,
    #[serde(default)]
    source: Option<zuse_protocol::slack::MessageSource>,
    #[serde(default)]
    workspace: Option<String>,
    #[serde(default)]
    session: Option<String>,
    #[serde(default)]
    progress_ts: Option<String>,
    #[serde(default)]
    replacement_for: Option<String>,
    #[serde(default)]
    result: Option<String>,
    #[serde(default)]
    submitted_at: Option<i64>,
    #[serde(default)]
    context: Vec<String>,
    #[serde(default)]
    context_cursor: Option<String>,
}
fn now() -> i64 {
    chrono::Utc::now().timestamp_millis()
}
fn hash(value: &str) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))
}
fn nonce() -> String {
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}
fn invalid() -> ApiError {
    ApiError::bad_request("invalid_slack_request")
}
fn unavailable() -> ApiError {
    ApiError::unavailable("slack_unavailable")
}
fn job_context(team: &str, id: &str) -> String {
    format!("zuse-slack-job-v1\n{team}\n{id}")
}
