//! Managed plugins: catalog and custom remote MCP servers, account connection
//! (OAuth, API key or public), discovery and tool calls, per workspace tenant.
//!
//! Tenants are `personal:<account>` or `organization:<id>`; the API derives them
//! from verified identity, never from a client-supplied value alone. Personal
//! connections stay private; organization-owned connections are shared with the
//! organization's members and machines. Upstream mutations are never retried
//! automatically, because a timeout may already have acted.
pub mod catalog;
pub mod egress;
mod executor;
pub(crate) mod routes;
mod views;
#[cfg(test)]
mod wakeup_tests;

use crate::{crypto::MasterKey, error::ApiError};
use catalog::Auth;
use egress::Policy;
use reqwest::{
    Url,
    header::{HeaderMap, HeaderName, HeaderValue},
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Row, postgres::PgRow};
use std::{sync::Arc, time::Duration};
use zuse_protocol::plugins::{
    PluginConnection, PluginConnectionState, PluginDefinition, PluginRequest, PluginResponse,
    PluginReturnTo, PluginTenant, PluginToolRequest,
};

/// `plugin-migrate` runs with DDL rights, so the tables it creates belong to
/// that role. The deployment's restricted application role (as in 0034) needs
/// read/write on the plugin metadata and the Executor SDK's tables.
const APP_ROLE_GRANTS: &str = r"DO $grants$
DECLARE schema_name text := current_schema(); app_role text; item record;
BEGIN
 app_role := CASE schema_name WHEN 'zuse_staging' THEN 'zuse_cloud_staging' WHEN 'zuse_production' THEN 'zuse_cloud_production' END;
 IF app_role IS NOT NULL AND EXISTS(SELECT 1 FROM pg_roles WHERE rolname=app_role) THEN
  FOR item IN SELECT tablename FROM pg_tables WHERE schemaname=schema_name AND (tablename LIKE 'executor\_%' OR tablename LIKE 'zuse\_plugin\_%' OR tablename='zuse_executor_state') LOOP
   EXECUTE format('GRANT SELECT,INSERT,UPDATE,DELETE ON TABLE %I.%I TO %I',schema_name,item.tablename,app_role);
  END LOOP;
  FOR item IN SELECT sequencename FROM pg_sequences WHERE schemaname=schema_name AND (sequencename LIKE 'executor\_%' OR sequencename LIKE 'zuse\_plugin\_%') LOOP
   EXECUTE format('GRANT USAGE,SELECT ON SEQUENCE %I.%I TO %I',schema_name,item.sequencename,app_role);
  END LOOP;
 END IF;
END $grants$;";
pub const MIGRATION: &str = include_str!("../migrations/0009_plugins.sql");
/// Change notifications that keep `serve`'s in-memory plugin views current.
pub const WAKEUPS_MIGRATION: &str = include_str!("../migrations/0046_plugin_wakeups.sql");
/// Change notifications for the SDK tables, which keep the executor's rows current.
pub const EXECUTOR_WAKEUPS_MIGRATION: &str =
    include_str!("../migrations/0048_executor_wakeups.sql");
pub const CALLBACK_PORTS: [u16; 4] = [8976, 8977, 8978, 8979];

const ATTEMPT_TTL_MS: i64 = 10 * 60_000;
const TERMINAL_RETENTION_MS: i64 = 24 * 60 * 60_000;
const MAX_CONNECTIONS: i64 = 50;
const MAX_SERVERS: i64 = 25;
const SEARCH_DEADLINE: Duration = Duration::from_secs(20);
const RESERVED_HEADERS: [&str; 8] = [
    "accept",
    "connection",
    "content-length",
    "content-type",
    "host",
    "mcp-protocol-version",
    "mcp-session-id",
    "transfer-encoding",
];

/// Stable codes clients can explain. Everything else is `plugin_operation_failed`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PluginFailure {
    OperationFailed,
    ClientRegistrationUnsupported,
    AuthUnsupported,
    Unreachable,
    UrlRejected,
    CredentialsRejected,
    LimitReached,
    SignInFailed,
}
impl PluginFailure {
    pub fn code(self) -> &'static str {
        match self {
            Self::OperationFailed => "plugin_operation_failed",
            Self::ClientRegistrationUnsupported => "plugin_client_registration_unsupported",
            Self::AuthUnsupported => "plugin_auth_unsupported",
            Self::Unreachable => "plugin_unreachable",
            Self::UrlRejected => "plugin_url_rejected",
            Self::CredentialsRejected => "plugin_credentials_rejected",
            Self::LimitReached => "plugin_limit_reached",
            Self::SignInFailed => "plugin_sign_in_failed",
        }
    }
}
impl From<PluginFailure> for ApiError {
    fn from(failure: PluginFailure) -> Self {
        ApiError::bad_request(failure.code())
    }
}

/// No Debug: holds the plugin credential key.
pub struct PluginConfig {
    pub key: MasterKey,
    /// Public API origin; provider callbacks return to `<origin>/v1/plugins/callback`.
    pub public_origin: Url,
    /// Hosted app that receives web OAuth returns.
    pub app_origin: Url,
    pub policy: Policy,
}

impl PluginConfig {
    pub fn from_env() -> anyhow::Result<Option<Self>> {
        let Ok(key) = std::env::var("ZUSE_CLOUD_PLUGIN_ENCRYPTION_KEY") else {
            return Ok(None);
        };
        let key = MasterKey::parse(&key)
            .map_err(|_| anyhow::anyhow!("ZUSE_CLOUD_PLUGIN_ENCRYPTION_KEY must be 32 bytes"))?;
        let origin = |name: &str| -> anyhow::Result<Url> {
            let raw = std::env::var(name).map_err(|_| anyhow::anyhow!("Plugins require {name}"))?;
            let url = Url::parse(&raw)?;
            anyhow::ensure!(
                url.scheme() == "https"
                    && url.host_str().is_some()
                    && url.username().is_empty()
                    && url.password().is_none()
                    && url.origin().ascii_serialization() == raw.trim_end_matches('/'),
                "{name} must be an HTTPS origin"
            );
            Ok(url)
        };
        Ok(Some(Self {
            key,
            public_origin: origin("ZUSE_CLOUD_PUBLIC_ORIGIN")?,
            app_origin: origin("ZUSE_CLOUD_PLUGIN_APP_ORIGIN")?,
            policy: Policy::Public,
        }))
    }
}

/// The verified tenant and the account acting in it.
#[derive(Clone, Debug)]
pub struct PluginIdentity {
    pub tenant: String,
    pub subject: String,
}
impl PluginIdentity {
    fn organization(&self) -> bool {
        self.tenant.starts_with("organization:")
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CustomServer {
    name: String,
    endpoint: String,
    #[serde(default)]
    header: Option<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AttemptSecret {
    label: String,
    #[serde(default)]
    return_to: Option<PluginReturnTo>,
    /// The provider's callback query, kept until the ticket is redeemed.
    #[serde(default)]
    query: Option<String>,
}

/// Resolved definition: catalog entry or tenant custom server.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Definition {
    id: String,
    name: String,
    endpoint: String,
    oauth_discovery: Option<String>,
    scopes: Option<Vec<String>>,
    auth: Option<Auth>,
    header: Option<String>,
}

struct Attempt {
    tenant: String,
    id: String,
    subject: String,
    plugin_id: String,
    state: String,
    error: Option<String>,
    authorization_url: Option<String>,
    sealed: String,
    ticket_digest: Option<String>,
    expires_at: i64,
}
impl Attempt {
    fn from_row(row: &PgRow) -> Self {
        Self {
            tenant: row.get("tenant_id"),
            id: row.get("attempt_id"),
            subject: row.get("subject"),
            plugin_id: row.get("plugin_id"),
            state: row.get("state"),
            error: row.get("error"),
            authorization_url: row.get("authorization_url"),
            sealed: row.get("sealed"),
            ticket_digest: row.get("ticket_digest"),
            expires_at: row.get("expires_at"),
        }
    }
    fn public(&self) -> PluginResponse {
        PluginResponse::Attempt {
            id: self.id.clone(),
            connection_id: self.id.clone(),
            state: self.state.clone(),
            authorization_url: (self.state == "pending")
                .then(|| self.authorization_url.clone())
                .flatten(),
            expires_at: self.expires_at as f64,
            error: (self.state == "failed").then(|| {
                self.error
                    .clone()
                    .unwrap_or_else(|| PluginFailure::OperationFailed.code().into())
            }),
            plugin_id: self.plugin_id.clone(),
        }
    }
}

#[derive(Clone)]
struct ConnectionRow {
    id: String,
    plugin_id: String,
    label: String,
    owner: String,
    subject: String,
    state: String,
    enabled: bool,
    auth: Option<String>,
    executor_ref: Option<String>,
    definition_hash: Option<String>,
    created_at: i64,
}
impl ConnectionRow {
    fn from_row(row: &PgRow) -> Self {
        Self {
            id: row.get("connection_id"),
            plugin_id: row.get("plugin_id"),
            label: row.get("label"),
            owner: row.get("owner"),
            subject: row.get("subject"),
            state: row.get("state"),
            enabled: row.get("enabled"),
            auth: row.get("auth"),
            executor_ref: row.get("executor_ref"),
            definition_hash: row.get("definition_hash"),
            created_at: row.get("created_at"),
        }
    }
    fn public(&self) -> PluginConnection {
        PluginConnection {
            id: self.id.clone(),
            plugin_id: self.plugin_id.clone(),
            label: self.label.clone(),
            owner: self.owner.clone(),
            state: match self.state.as_str() {
                "connecting" => PluginConnectionState::Connecting,
                "connected" => PluginConnectionState::Connected,
                "needs-auth" => PluginConnectionState::NeedsAuth,
                _ => PluginConnectionState::Error,
            },
            created_at: self.created_at as f64,
            enabled: self.enabled,
        }
    }
    fn prefix(&self) -> String {
        format!("tools.{}.user.{}.", self.plugin_id, self.id)
    }
}

pub struct PluginControl {
    pool: PgPool,
    config: PluginConfig,
    http: reqwest::Client,
    executor: tokio::sync::Mutex<Option<Arc<executor::Executor>>>,
    gate: Arc<tokio::sync::Semaphore>,
    /// Connection rows and custom servers per tenant; see [`views`].
    views: views::Views,
    fences: views::Fences,
}

/// Holds an admitted operation's capacity permit and tenant fences.
struct Admitted {
    _permit: tokio::sync::OwnedSemaphorePermit,
    _local: tokio::sync::OwnedMutexGuard<()>,
    /// The cross-process advisory lock of management operations.
    _shared: Option<sqlx::Transaction<'static, sqlx::Postgres>>,
}

fn now() -> i64 {
    chrono::Utc::now().timestamp_millis()
}
fn digest(value: &str) -> String {
    let hash = Sha256::digest(value.as_bytes());
    hash.iter().map(|b| format!("{b:02x}")).collect()
}
fn invalid() -> ApiError {
    ApiError::bad_request("invalid_plugin_request")
}
fn failed() -> ApiError {
    PluginFailure::OperationFailed.into()
}
fn valid_uuid(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(i, b)| match i {
            8 | 13 | 18 | 23 => b == b'-',
            _ => b.is_ascii_digit() || (b'a'..=b'f').contains(&b),
        })
}
fn valid_header_name(value: &str) -> bool {
    (1..=64).contains(&value.len())
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+.^_`|~-".contains(&b))
        && !RESERVED_HEADERS.contains(&value.to_ascii_lowercase().as_str())
}
fn valid_key(value: &str) -> bool {
    !value.is_empty() && value.len() <= 4096 && !value.contains(['\r', '\n', '\0'])
}
fn rejected(id: &str, plugin_id: &str, failure: PluginFailure) -> PluginResponse {
    PluginResponse::Attempt {
        id: id.into(),
        connection_id: id.into(),
        state: "failed".into(),
        authorization_url: None,
        expires_at: now() as f64,
        error: Some(failure.code().into()),
        plugin_id: plugin_id.into(),
    }
}
fn public_server(id: &str, server: &CustomServer) -> Option<PluginDefinition> {
    let url = Url::parse(&server.endpoint).ok()?;
    let host = url.host_str()?.to_owned();
    let domain = host
        .strip_prefix("mcp.")
        .or_else(|| host.strip_prefix("api."))
        .unwrap_or(&host)
        .to_owned();
    Some(PluginDefinition {
        command: None,
        id: id.into(),
        name: server.name.clone(),
        description: url.host_str()?.to_owned()
            + &url.port().map(|p| format!(":{p}")).unwrap_or_default(),
        domain,
        category: None,
        featured: false,
        custom: true,
        // Never the query string: URLs can carry tokens.
        url: Some(format!(
            "{}{}",
            url.origin().ascii_serialization(),
            url.path()
        )),
        api_key_header: server.header.clone(),
    })
}

impl PluginControl {
    pub async fn migrate_url(url: &str, key: &MasterKey) -> Result<(), ApiError> {
        let pool = sqlx::PgPool::connect(url).await?;
        Self::migrate(&pool, key).await
    }
    pub async fn migrate(pool: &PgPool, key: &MasterKey) -> Result<(), ApiError> {
        let mut lock = pool.begin().await?;
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended('zuse-executor-schema-v1',0))")
            .execute(&mut *lock)
            .await?;
        sqlx::raw_sql(MIGRATION).execute(pool).await?;
        sqlx::raw_sql(WAKEUPS_MIGRATION).execute(pool).await?;
        executor::Executor::migrate(pool, key).await?;
        sqlx::raw_sql(EXECUTOR_WAKEUPS_MIGRATION)
            .execute(pool)
            .await?;
        sqlx::raw_sql(APP_ROLE_GRANTS).execute(pool).await?;
        lock.commit().await?;
        Ok(())
    }

    pub fn from_pool(pool: PgPool, config: PluginConfig) -> anyhow::Result<Arc<Self>> {
        Ok(Arc::new(Self {
            http: config.policy.client()?,
            pool,
            config,
            executor: tokio::sync::Mutex::new(None),
            gate: Arc::new(tokio::sync::Semaphore::new(2)),
            views: views::Views::new(views::VIEW_TTL),
            fences: views::Fences::default(),
        }))
    }

    /// Keeps tenants' connection rows and custom servers in memory while
    /// database notifications can invalidate them: `LISTEN` is up and the
    /// `0046_plugin_wakeups.sql` triggers exist. Otherwise every operation
    /// reads them from the database, as before.
    pub async fn attach_wakeups(self: &Arc<Self>, wakeups: Arc<crate::db::wake::Wakeups>) {
        if !wakeups.enabled() {
            return;
        }
        // Subscribe first so no change between the check and enabling is missed.
        let mut notifications = wakeups.subscribe();
        let installed: Result<i64, sqlx::Error> = sqlx::query_scalar("SELECT count(*) FROM pg_trigger WHERE NOT tgisinternal AND tgenabled<>'D' AND ((tgname='zuse_cloud_plugin_connections_wake' AND tgrelid=to_regclass('zuse_plugin_connections')) OR (tgname='zuse_cloud_plugin_servers_wake' AND tgrelid=to_regclass('zuse_plugin_servers')))")
            .fetch_one(&self.pool)
            .await;
        if !matches!(installed, Ok(2)) {
            tracing::warn!(
                code = "plugin_wakeups_missing",
                "Plugin change notifications are not installed; reading plugin connections on every call"
            );
            return;
        }
        self.views.enable();
        let weak = Arc::downgrade(self);
        tokio::spawn(async move {
            let _wakeups = wakeups;
            use crate::db::wake::Work;
            use tokio::sync::broadcast::error::RecvError;
            loop {
                let work = notifications.recv().await;
                let Some(control) = weak.upgrade() else { break };
                match work {
                    Ok(Work::Plugins | Work::All) | Err(RecvError::Lagged(_)) => {
                        control.views.clear()
                    }
                    Ok(_) => {}
                    Err(RecvError::Closed) => {
                        control.views.disable();
                        break;
                    }
                }
            }
        });
    }

    /// The tenant's connection rows and custom servers, from memory when
    /// notifications keep them current.
    async fn view(&self, tenant: &str) -> Result<views::View, ApiError> {
        if let Some(view) = self.views.get(tenant) {
            return Ok(view);
        }
        let generation = self.views.generation();
        let rows = sqlx::query(
            "SELECT * FROM zuse_plugin_connections WHERE tenant_id=$1 ORDER BY created_at,connection_id",
        )
        .bind(tenant)
        .fetch_all(&self.pool)
        .await?;
        let servers = self
            .servers(tenant)
            .await?
            .into_iter()
            .map(|(id, server, _)| (id, server))
            .collect();
        let view = views::View {
            rows: Arc::new(rows.iter().map(ConnectionRow::from_row).collect()),
            servers: Arc::new(servers),
        };
        self.views.store(tenant, generation, view.clone());
        Ok(view)
    }

    pub async fn ready(&self) -> Result<(), ApiError> {
        let ready: bool = sqlx::query_scalar("SELECT to_regclass('zuse_plugin_connections') IS NOT NULL AND to_regclass('zuse_plugin_attempts') IS NOT NULL AND to_regclass('zuse_plugin_servers') IS NOT NULL")
            .fetch_one(&self.pool)
            .await?;
        if ready {
            self.engine(json!({"operation":"ready"})).await?;
            Ok(())
        } else {
            Err(ApiError::unavailable("cloud_plugin_schema_not_ready"))
        }
    }

    fn callback_url(&self) -> String {
        format!(
            "{}/v1/plugins/callback",
            self.config.public_origin.origin().ascii_serialization()
        )
    }

    fn aad(kind: &str, tenant: &str, id: &str) -> Vec<u8> {
        format!("zuse-plugins:v1:{kind}:{tenant}:{id}").into_bytes()
    }
    fn seal<T: Serialize>(
        &self,
        kind: &str,
        tenant: &str,
        id: &str,
        value: &T,
    ) -> Result<String, ApiError> {
        let plaintext = zeroize::Zeroizing::new(serde_json::to_vec(value).map_err(|_| failed())?);
        self.config
            .key
            .seal(&Self::aad(kind, tenant, id), &plaintext)
            .map_err(|_| ApiError::unavailable("cloud_plugin_encryption_failed"))
    }
    fn open<T: DeserializeOwned>(
        &self,
        kind: &str,
        tenant: &str,
        id: &str,
        sealed: &str,
    ) -> Result<T, ApiError> {
        let plaintext = self
            .config
            .key
            .open(&Self::aad(kind, tenant, id), sealed)
            .map_err(|_| ApiError::unavailable("cloud_plugin_decryption_failed"))?;
        serde_json::from_slice(&plaintext).map_err(|_| failed())
    }

    async fn servers(&self, tenant: &str) -> Result<Vec<(String, CustomServer, i64)>, ApiError> {
        let rows = sqlx::query("SELECT server_id,sealed,created_at FROM zuse_plugin_servers WHERE tenant_id=$1 ORDER BY created_at,server_id")
            .bind(tenant)
            .fetch_all(&self.pool)
            .await?;
        rows.iter()
            .map(|row| {
                let id: String = row.get("server_id");
                let server = self.open("server", tenant, &id, row.get("sealed"))?;
                Ok((id, server, row.get("created_at")))
            })
            .collect()
    }

    fn catalog_definition(id: &str) -> Option<Definition> {
        catalog::find(id).map(|entry| Definition {
            id: entry.id.clone(),
            name: entry.name.clone(),
            endpoint: entry.endpoint.clone(),
            oauth_discovery: entry.oauth_discovery.clone(),
            scopes: entry.scopes.clone(),
            auth: entry.auth,
            header: None,
        })
    }
    fn custom_definition(id: &str, server: CustomServer) -> Definition {
        Definition {
            id: id.into(),
            name: server.name,
            endpoint: server.endpoint,
            oauth_discovery: None,
            scopes: None,
            auth: server.header.is_some().then_some(Auth::Header),
            header: server.header,
        }
    }
    /// [`Self::definition`] resolved from a tenant view.
    fn view_definition(view: &views::View, id: &str) -> Option<Definition> {
        if let Some(definition) = Self::catalog_definition(id) {
            return Some(definition);
        }
        if !id.starts_with("custom-") {
            return None;
        }
        view.servers
            .iter()
            .find(|(server_id, _)| server_id == id)
            .map(|(_, server)| Self::custom_definition(id, server.clone()))
    }

    async fn definition(&self, tenant: &str, id: &str) -> Result<Option<Definition>, ApiError> {
        if let Some(definition) = Self::catalog_definition(id) {
            return Ok(Some(definition));
        }
        if !id.starts_with("custom-") {
            return Ok(None);
        }
        let sealed: Option<String> = sqlx::query_scalar(
            "SELECT sealed FROM zuse_plugin_servers WHERE tenant_id=$1 AND server_id=$2",
        )
        .bind(tenant)
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        let Some(sealed) = sealed else {
            return Ok(None);
        };
        let server: CustomServer = self.open("server", tenant, id, &sealed)?;
        Ok(Some(Self::custom_definition(id, server)))
    }

    /// Personal tenants see their own rows; organization tenants also see rows
    /// the organization owns. Legacy-style user-owned rows stay private.
    async fn visible(&self, identity: &PluginIdentity) -> Result<Vec<ConnectionRow>, ApiError> {
        let rows = sqlx::query("SELECT * FROM zuse_plugin_connections WHERE tenant_id=$1 AND (subject=$2 OR owner='organization') ORDER BY created_at,connection_id")
            .bind(&identity.tenant)
            .bind(&identity.subject)
            .fetch_all(&self.pool)
            .await?;
        Ok(rows.iter().map(ConnectionRow::from_row).collect())
    }

    /// [`Self::visible`] from the tenant view.
    async fn visible_cached(
        &self,
        identity: &PluginIdentity,
    ) -> Result<(views::View, Vec<ConnectionRow>), ApiError> {
        let view = self.view(&identity.tenant).await?;
        let rows = view
            .rows
            .iter()
            .filter(|row| row.subject == identity.subject || row.owner == "organization")
            .cloned()
            .collect();
        Ok((view, rows))
    }

    async fn attempt(&self, tenant: &str, id: &str) -> Result<Option<Attempt>, ApiError> {
        Ok(
            sqlx::query("SELECT * FROM zuse_plugin_attempts WHERE tenant_id=$1 AND attempt_id=$2")
                .bind(tenant)
                .bind(id)
                .fetch_optional(&self.pool)
                .await?
                .as_ref()
                .map(Attempt::from_row),
        )
    }

    /// Keep an admitted operation's tenant fence until it settles, including
    /// when the HTTP caller disconnects. Dropping a request cannot release the
    /// lock ahead of a worker that may already be renewing credentials.
    pub async fn request(
        self: &Arc<Self>,
        identity: &PluginIdentity,
        tenant: PluginTenant,
        can_manage: bool,
        input: PluginRequest,
    ) -> Result<PluginResponse, ApiError> {
        let control = self.clone();
        let identity = identity.clone();
        tokio::spawn(async move {
            control
                .request_inner(&identity, tenant, can_manage, input)
                .await
        })
        .await
        .map_err(|_| ApiError::unavailable("plugin_operation_interrupted"))?
    }
    async fn request_inner(
        &self,
        identity: &PluginIdentity,
        tenant: PluginTenant,
        can_manage: bool,
        input: PluginRequest,
    ) -> Result<PluginResponse, ApiError> {
        if input.tenant_id().is_some_and(|id| id != identity.tenant) {
            return Err(ApiError::forbidden("plugin_tenant_rejected"));
        }
        if !can_manage && !matches!(&input, PluginRequest::List { .. }) {
            return Err(ApiError::forbidden("workspace_access_denied"));
        }
        let _guard = self.admit(&identity.tenant).await?;
        let _changes = (!matches!(&input, PluginRequest::List { .. }))
            .then(|| self.views.changes(&identity.tenant));
        match input {
            PluginRequest::List { .. } => {
                let (view, visible) = self.visible_cached(identity).await?;
                let mut catalog: Vec<PluginDefinition> = view
                    .servers
                    .iter()
                    .filter_map(|(id, server)| public_server(id, server))
                    .collect();
                catalog.extend_from_slice(catalog::public());
                let tenant_path: String =
                    url::form_urlencoded::byte_serialize(identity.tenant.as_bytes()).collect();
                Ok(PluginResponse::Snapshot {
                    can_manage,
                    tenants: vec![tenant],
                    tenant_id: identity.tenant.clone(),
                    endpoint: format!(
                        "{}/v1/plugins/{tenant_path}/mcp",
                        self.config.public_origin.origin().ascii_serialization()
                    ),
                    catalog,
                    connections: visible.iter().map(ConnectionRow::public).collect(),
                    moved: Vec::new(),
                })
            }
            PluginRequest::Connect {
                plugin_id,
                label,
                request_id,
                return_to,
                api_key,
                ..
            } => {
                self.connect_plugin(
                    identity,
                    &plugin_id,
                    &label,
                    &request_id,
                    return_to,
                    api_key,
                )
                .await
            }
            PluginRequest::AddServer {
                request_id,
                name,
                url,
                api_key,
                return_to,
                ..
            } => {
                self.add_server(identity, &request_id, &name, &url, api_key, return_to)
                    .await
            }
            PluginRequest::RemoveServer { plugin_id, .. } => {
                self.remove_server(identity, &plugin_id).await
            }
            PluginRequest::Poll { attempt_id, .. } => self.poll(identity, &attempt_id, false).await,
            PluginRequest::Cancel { attempt_id, .. } => {
                self.poll(identity, &attempt_id, true).await
            }
            PluginRequest::Complete { ticket, .. } => self.complete(identity, &ticket).await,
            PluginRequest::Disconnect { connection_id, .. } => {
                let row = self.visible_row(identity, &connection_id).await?;
                self.remove_connection(&identity.tenant, &row.id).await?;
                Ok(PluginResponse::Ok)
            }
            // Local servers live on the user's device and never reach the cloud.
            PluginRequest::AddLocalServer { .. } => Err(ApiError::bad_request("plugin_local_only")),
            PluginRequest::Rename {
                connection_id,
                label,
                ..
            } => {
                let row = self.visible_row(identity, &connection_id).await?;
                let label = label.trim();
                if label.is_empty() || label.chars().count() > 80 {
                    return Err(invalid());
                }
                let duplicate:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM zuse_plugin_connections WHERE tenant_id=$1 AND plugin_id=$2 AND connection_id<>$3 AND lower(trim(label))=lower($4))").bind(&identity.tenant).bind(&row.plugin_id).bind(&row.id).bind(label).fetch_one(&self.pool).await?;
                if duplicate {
                    return Err(ApiError::bad_request("plugin_label_exists"));
                }
                sqlx::query("UPDATE zuse_plugin_connections SET label=$3,updated_at=$4 WHERE tenant_id=$1 AND connection_id=$2").bind(&identity.tenant).bind(&row.id).bind(label).bind(now()).execute(&self.pool).await?;
                Ok(PluginResponse::Ok)
            }
            PluginRequest::SetEnabled {
                connection_id,
                enabled,
                ..
            } => {
                let row = self.visible_row(identity, &connection_id).await?;
                sqlx::query("UPDATE zuse_plugin_connections SET enabled=$3,updated_at=$4 WHERE tenant_id=$1 AND connection_id=$2")
                    .bind(&identity.tenant)
                    .bind(&row.id)
                    .bind(enabled)
                    .bind(now())
                    .execute(&self.pool)
                    .await?;
                Ok(PluginResponse::Ok)
            }
        }
    }

    async fn visible_row(
        &self,
        identity: &PluginIdentity,
        id: &str,
    ) -> Result<ConnectionRow, ApiError> {
        self.visible(identity)
            .await?
            .into_iter()
            .find(|row| row.id == id)
            .ok_or_else(failed)
    }

    async fn remove_connection(&self, tenant: &str, id: &str) -> Result<(), ApiError> {
        sqlx::query("UPDATE zuse_plugin_connections SET state='error' WHERE tenant_id=$1 AND connection_id=$2").bind(tenant).bind(id).execute(&self.pool).await?;
        self.remove_executor(tenant, id).await?;
        let mut tx = self.pool.begin().await?;
        sqlx::query("DELETE FROM zuse_plugin_connections WHERE tenant_id=$1 AND connection_id=$2")
            .bind(tenant)
            .bind(id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("UPDATE zuse_plugin_attempts SET state='cancelled',error=NULL,authorization_url=NULL,state_digest=NULL,ticket_digest=NULL,updated_at=$3 WHERE tenant_id=$1 AND attempt_id=$2 AND state='pending'")
            .bind(tenant)
            .bind(id)
            .bind(now())
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    async fn remove_executor(&self, tenant: &str, id: &str) -> Result<(), ApiError> {
        let sealed:Option<String>=sqlx::query_scalar("SELECT executor_ref FROM zuse_plugin_connections WHERE tenant_id=$1 AND connection_id=$2").bind(tenant).bind(id).fetch_optional(&self.pool).await?.flatten();
        let reference: Option<Value> = sealed
            .as_deref()
            .map(|sealed| self.open("executor", tenant, id, sealed))
            .transpose()?;
        self.engine(json!({"operation":"remove","owner":tenant,"id":id,"ref":reference}))
            .await?;

        Ok(())
    }
    fn return_target(&self, return_to: Option<&PluginReturnTo>) -> Result<Url, ApiError> {
        match return_to {
            Some(PluginReturnTo::Desktop { port }) if CALLBACK_PORTS.contains(port) => {
                Url::parse(&format!("http://127.0.0.1:{port}/plugins/callback"))
                    .map_err(|_| failed())
            }
            Some(PluginReturnTo::Desktop { .. }) => Err(invalid()),
            _ => Ok(self.config.app_origin.clone()),
        }
    }

    async fn connect_plugin(
        &self,
        identity: &PluginIdentity,
        plugin_id: &str,
        label: &str,
        request_id: &str,
        return_to: Option<PluginReturnTo>,
        api_key: Option<String>,
    ) -> Result<PluginResponse, ApiError> {
        if !valid_uuid(request_id) || label.chars().count() > 80 {
            return Err(invalid());
        }
        let id = format!("c{}", request_id.replace('-', ""));
        if let Some(old) = self.attempt(&identity.tenant, &id).await? {
            return if old.subject == identity.subject {
                Ok(old.public())
            } else {
                Err(invalid())
            };
        }
        let definition = self
            .definition(&identity.tenant, plugin_id)
            .await?
            .ok_or_else(invalid)?;
        self.return_target(return_to.as_ref())?;
        if definition.auth == Some(Auth::Header) && !api_key.as_deref().is_some_and(valid_key) {
            return Ok(rejected(
                &id,
                &definition.id,
                PluginFailure::CredentialsRejected,
            ));
        }
        let label = match label.trim() {
            "" => definition.name.clone(),
            label => label.to_owned(),
        };
        let at = now();
        let secret = AttemptSecret {
            label: label.clone(),
            return_to,
            query: None,
        };
        let sealed = self.seal("attempt", &identity.tenant, &id, &secret)?;
        let mut tx = self.pool.begin().await?;
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
            .bind(format!("plugins:{}", identity.tenant))
            .execute(&mut *tx)
            .await?;
        let existing: i64 = sqlx::query_scalar("SELECT count(*) FROM zuse_plugin_connections WHERE tenant_id=$1 AND ($2 OR subject=$3)")
            .bind(&identity.tenant)
            .bind(identity.organization())
            .bind(&identity.subject)
            .fetch_one(&mut *tx)
            .await?;
        if existing >= MAX_CONNECTIONS {
            return Ok(rejected(&id, &definition.id, PluginFailure::LimitReached));
        }
        let inserted = sqlx::query("INSERT INTO zuse_plugin_attempts(tenant_id,attempt_id,subject,plugin_id,state,sealed,expires_at,created_at,updated_at) VALUES($1,$2,$3,$4,'pending',$5,$6,$7,$7) ON CONFLICT DO NOTHING")
            .bind(&identity.tenant)
            .bind(&id)
            .bind(&identity.subject)
            .bind(&definition.id)
            .bind(&sealed)
            .bind(at + ATTEMPT_TTL_MS)
            .bind(at)
            .execute(&mut *tx)
            .await?
            .rows_affected();
        if inserted == 0 {
            drop(tx);
            return Box::pin(self.connect_plugin(
                identity,
                plugin_id,
                &label,
                request_id,
                secret.return_to,
                api_key,
            ))
            .await;
        }
        let owner = if identity.organization() {
            "organization"
        } else {
            "user"
        };
        let created = sqlx::query("INSERT INTO zuse_plugin_connections(tenant_id,connection_id,subject,plugin_id,label,owner,state,created_at,updated_at) VALUES($1,$2,$3,$4,$5,$6,'connecting',$7,$7) ON CONFLICT DO NOTHING")
            .bind(&identity.tenant)
            .bind(&id)
            .bind(&identity.subject)
            .bind(&definition.id)
            .bind(&label)
            .bind(owner)
            .bind(at)
            .execute(&mut *tx)
            .await?
            .rows_affected();
        if created == 0 {
            return Err(invalid());
        }
        tx.commit().await?;

        let result = self
            .establish(identity, &id, &definition, &secret, api_key.as_deref())
            .await;
        match result {
            Ok(()) => {}
            Err(failure) => {
                tracing::warn!(plugin = %definition.id, code = failure.code(), "Plugin connect failed");
                self.fail(&identity.tenant, &id, "failed", Some(failure))
                    .await?;
            }
        }
        self.attempt(&identity.tenant, &id)
            .await?
            .map(|a| a.public())
            .ok_or_else(failed)
    }

    async fn establish(
        &self,
        identity: &PluginIdentity,
        id: &str,
        definition: &Definition,
        secret: &AttemptSecret,
        api_key: Option<&str>,
    ) -> Result<(), PluginFailure> {
        let auth = match definition.auth {
            Some(auth) => auth,
            None => {
                let probe = egress::probe(
                    &self.http,
                    self.config.policy,
                    &definition.endpoint,
                    HeaderMap::new(),
                )
                .await?;
                match probe.status.as_u16() {
                    200..=299 => Auth::None,
                    401 | 403 => Auth::Oauth,
                    _ => return Err(PluginFailure::AuthUnsupported),
                }
            }
        };
        let execute = async {
            let mut reference = self.engine(json!({"operation":"prepare","owner":identity.tenant,"subject":identity.subject,"id":id,"definition":definition,"auth":auth.as_str()})).await?;
            self.save_ref(&identity.tenant,id,definition,auth,&reference).await?;
            if auth == Auth::Header {
                let header=definition.header.as_deref().unwrap_or("Authorization");
                let value=api_key.filter(|v|valid_key(v)).ok_or_else(||ApiError::from(PluginFailure::CredentialsRejected))?;
                let mut headers=HeaderMap::new();
                headers.insert(HeaderName::from_bytes(header.as_bytes()).map_err(|_|invalid())?,HeaderValue::from_str(value).map_err(|_|invalid())?);
                let probe=egress::probe(&self.http,self.config.policy,&definition.endpoint,headers).await?;
                if !probe.status.is_success() {return Err(PluginFailure::CredentialsRejected.into());}
                reference=self.engine(json!({"operation":"submit","owner":identity.tenant,"ref":reference,"definition":definition,"label":secret.label,"value":value})).await?;
                self.save_ref(&identity.tenant,id,definition,auth,&reference).await?;
            }
            if auth != Auth::Oauth {
                // SDK negotiation and catalog validation establish that the server is usable.
                self.engine(json!({"operation":"list","owner":identity.tenant,"ref":reference,"definition":definition})).await?;
                self.finish(&identity.tenant,id,auth,&secret.label,secret.return_to.clone()).await?;
                return Ok::<(),ApiError>(());
            }
            let authorization=self.engine(json!({"operation":"start","owner":identity.tenant,"ref":reference,"definition":definition,"label":secret.label,"redirectUri":self.callback_url()})).await?;
            let url=self.config.policy.parse(authorization.as_str().ok_or_else(failed)?)?;
            let state=url.query_pairs().find(|(k,_)|k=="state").map(|(_,v)|v.into_owned()).ok_or_else(failed)?;
            let updated=sqlx::query("UPDATE zuse_plugin_attempts SET authorization_url=$3,state_digest=$4,updated_at=$5 WHERE tenant_id=$1 AND attempt_id=$2 AND state='pending' AND expires_at>$5")
                .bind(&identity.tenant).bind(id).bind(url.as_str()).bind(digest(&state)).bind(now()).execute(&self.pool).await?.rows_affected();
            if updated!=1 {return Err(failed());}
            Ok(())
        }.await;
        execute.map_err(|error| match error.code {
            "plugin_client_registration_unsupported" => {
                PluginFailure::ClientRegistrationUnsupported
            }
            "plugin_auth_unsupported" => PluginFailure::AuthUnsupported,
            "plugin_unreachable" => PluginFailure::Unreachable,
            "plugin_credentials_rejected" => PluginFailure::CredentialsRejected,
            _ => PluginFailure::OperationFailed,
        })
    }

    async fn engine(&self, mut request: Value) -> Result<Value, ApiError> {
        // The executor keeps SDK rows only while this process can forward
        // change notifications; a new epoch drops them (see `views`).
        if let Some(fields) = request.as_object_mut() {
            fields.insert(
                "epoch".into(),
                self.views.epoch().map_or(Value::Null, Value::from),
            );
        }
        let mut slot = self.executor.lock().await;
        if let Some(engine) = slot.as_ref() {
            if !engine.alive().await {
                *slot = None;
            }
        }
        if slot.is_none() {
            *slot = Some(
                executor::Executor::start(&self.pool, &self.config.key, self.config.policy).await?,
            );
        }
        let engine = slot.as_ref().expect("initialized executor").clone();
        drop(slot);
        // A failed request is never replayed. A later request may start a fresh
        // worker, whose app/account state is retained by the actual SDK in PG.
        engine.call(request).await
    }

    fn definition_hash(definition: &Definition, auth: Auth) -> Result<String, ApiError> {
        Ok(digest(
            &serde_json::to_string(&(definition, auth.as_str())).map_err(|_| failed())?,
        ))
    }
    async fn save_ref(
        &self,
        tenant: &str,
        id: &str,
        definition: &Definition,
        auth: Auth,
        reference: &Value,
    ) -> Result<(), ApiError> {
        let sealed = self.seal("executor", tenant, id, reference)?;
        let hash = Self::definition_hash(definition, auth)?;
        let updated=sqlx::query("UPDATE zuse_plugin_connections SET executor_ref=$3,definition_hash=$4,auth=$5,credential=NULL WHERE tenant_id=$1 AND connection_id=$2")
            .bind(tenant).bind(id).bind(sealed).bind(hash).bind(auth.as_str()).execute(&self.pool).await?.rows_affected();
        if updated != 1 {
            return Err(failed());
        }
        Ok(())
    }
    fn reference(
        &self,
        tenant: &str,
        row: &ConnectionRow,
        definition: &Definition,
    ) -> Result<Value, ApiError> {
        let auth = row
            .auth
            .as_deref()
            .and_then(Auth::parse)
            .ok_or_else(failed)?;
        if row.definition_hash.as_deref() != Some(Self::definition_hash(definition, auth)?.as_str())
        {
            return Err(ApiError::conflict(
                "plugin_definition_changed_reconnect_required",
            ));
        }
        self.open(
            "executor",
            tenant,
            &row.id,
            row.executor_ref
                .as_deref()
                .ok_or_else(|| ApiError::conflict("plugin_executor_reconnect_required"))?,
        )
    }
    /// Admits a management operation: a capacity permit, this process's
    /// tenant fence and the cross-process advisory lock, held until it settles.
    async fn admit(&self, tenant: &str) -> Result<Admitted, ApiError> {
        let mut admitted = self.admit_local(tenant).await?;
        let mut tx = self.pool.begin().await?;
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
            .bind(format!("plugin-operation:{tenant}"))
            .execute(&mut *tx)
            .await?;
        admitted._shared = Some(tx);
        Ok(admitted)
    }
    /// Admits a tool operation: a capacity permit and this process's tenant
    /// fence. Tool operations change no plugin rows except marking a refused
    /// connection, and the SDK claims credential renewals in its own rows, so
    /// they hold no database connection while the upstream server runs.
    async fn admit_local(&self, tenant: &str) -> Result<Admitted, ApiError> {
        let permit = self
            .gate
            .clone()
            .try_acquire_owned()
            .map_err(|_| ApiError::unavailable("plugins_busy"))?;
        Ok(Admitted {
            _permit: permit,
            _local: self.fences.lock(tenant).await,
            _shared: None,
        })
    }
    /// Marks the connection usable and clears attempt secrets atomically.
    async fn finish(
        &self,
        tenant: &str,
        id: &str,
        auth: Auth,
        label: &str,
        return_to: Option<PluginReturnTo>,
    ) -> Result<(), ApiError> {
        let cleared = self.seal(
            "attempt",
            tenant,
            id,
            &AttemptSecret {
                label: label.into(),
                return_to,
                query: None,
            },
        )?;
        let at = now();
        let mut tx = self.pool.begin().await?;
        let attempt = sqlx::query("UPDATE zuse_plugin_attempts SET state='connected',error=NULL,authorization_url=NULL,state_digest=NULL,ticket_digest=NULL,sealed=$3,updated_at=$4 WHERE tenant_id=$1 AND attempt_id=$2 AND state='pending' AND expires_at>$4")
            .bind(tenant)
            .bind(id)
            .bind(cleared)
            .bind(at)
            .execute(&mut *tx)
            .await?
            .rows_affected();
        let connection = sqlx::query("UPDATE zuse_plugin_connections SET state='connected',auth=$3,credential=NULL,credential_revision=credential_revision+1,updated_at=$4 WHERE tenant_id=$1 AND connection_id=$2 AND state='connecting'")
            .bind(tenant)
            .bind(id)
            .bind(auth.as_str())
            .bind(at)
            .execute(&mut *tx)
            .await?
            .rows_affected();
        if attempt != 1 || connection != 1 {
            // Cancelled, expired or disconnected meanwhile: keep nothing.
            return Err(failed());
        }
        tx.commit().await?;
        Ok(())
    }

    /// Ends a pending attempt and removes its unfinished connection.
    async fn fail(
        &self,
        tenant: &str,
        id: &str,
        state: &str,
        failure: Option<PluginFailure>,
    ) -> Result<(), ApiError> {
        let at = now();
        let mut tx = self.pool.begin().await?;
        let row = sqlx::query("SELECT sealed FROM zuse_plugin_attempts WHERE tenant_id=$1 AND attempt_id=$2 AND state='pending' FOR UPDATE")
            .bind(tenant)
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?;
        let Some(row) = row else {
            return Ok(());
        };
        self.remove_executor(tenant, id).await?;
        let secret: AttemptSecret = self.open("attempt", tenant, id, row.get("sealed"))?;
        let cleared = self.seal(
            "attempt",
            tenant,
            id,
            &AttemptSecret {
                label: secret.label,
                return_to: secret.return_to,
                query: None,
            },
        )?;
        sqlx::query("UPDATE zuse_plugin_attempts SET state=$3,error=$4,authorization_url=NULL,state_digest=NULL,ticket_digest=NULL,sealed=$5,updated_at=$6 WHERE tenant_id=$1 AND attempt_id=$2")
            .bind(tenant)
            .bind(id)
            .bind(state)
            .bind(
                (state == "failed")
                    .then(|| failure.unwrap_or(PluginFailure::OperationFailed).code()),
            )
            .bind(cleared)
            .bind(at)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM zuse_plugin_connections WHERE tenant_id=$1 AND connection_id=$2 AND state='connecting'")
            .bind(tenant)
            .bind(id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    async fn add_server(
        &self,
        identity: &PluginIdentity,
        request_id: &str,
        name: &str,
        url: &str,
        api_key: Option<zuse_protocol::plugins::PluginApiKey>,
        return_to: Option<PluginReturnTo>,
    ) -> Result<PluginResponse, ApiError> {
        if !valid_uuid(request_id) {
            return Err(invalid());
        }
        let hex = request_id.replace('-', "");
        let id = format!("custom-{hex}");
        let attempt_id = format!("c{hex}");
        if let Some(old) = self.attempt(&identity.tenant, &attempt_id).await? {
            return if old.subject == identity.subject {
                Ok(old.public())
            } else {
                Err(invalid())
            };
        }
        let name = name.trim();
        let url = url.trim();
        if name.is_empty()
            || name.chars().count() > 60
            || api_key
                .as_ref()
                .is_some_and(|k| !valid_header_name(&k.header))
        {
            return Err(invalid());
        }
        if url.len() > 2048 || self.config.policy.parse(url).is_err() {
            return Ok(rejected(&attempt_id, &id, PluginFailure::UrlRejected));
        }
        if api_key.as_ref().is_some_and(|k| !valid_key(&k.value)) {
            return Ok(rejected(
                &attempt_id,
                &id,
                PluginFailure::CredentialsRejected,
            ));
        }
        self.return_target(return_to.as_ref())?;
        let server = CustomServer {
            name: name.into(),
            endpoint: url.into(),
            header: api_key.as_ref().map(|k| k.header.clone()),
        };
        let sealed = self.seal("server", &identity.tenant, &id, &server)?;
        let mut tx = self.pool.begin().await?;
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
            .bind(format!("plugins:{}", identity.tenant))
            .execute(&mut *tx)
            .await?;
        let count: i64 =
            sqlx::query_scalar("SELECT count(*) FROM zuse_plugin_servers WHERE tenant_id=$1")
                .bind(&identity.tenant)
                .fetch_one(&mut *tx)
                .await?;
        if count >= MAX_SERVERS {
            return Ok(rejected(&attempt_id, &id, PluginFailure::LimitReached));
        }
        sqlx::query("INSERT INTO zuse_plugin_servers(tenant_id,server_id,sealed,created_at) VALUES($1,$2,$3,$4) ON CONFLICT DO NOTHING")
            .bind(&identity.tenant)
            .bind(&id)
            .bind(sealed)
            .bind(now())
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        let attempt = self
            .connect_plugin(
                identity,
                &id,
                name,
                request_id,
                return_to,
                api_key.map(|k| k.value),
            )
            .await?;
        if matches!(&attempt, PluginResponse::Attempt { state, .. } if state == "failed") {
            // The first connection failed: keep nothing so the user can fix it and retry.
            sqlx::query("DELETE FROM zuse_plugin_servers WHERE tenant_id=$1 AND server_id=$2 AND NOT EXISTS(SELECT 1 FROM zuse_plugin_connections WHERE tenant_id=$1 AND plugin_id=$2)")
                .bind(&identity.tenant)
                .bind(&id)
                .execute(&self.pool)
                .await?;
        }
        Ok(attempt)
    }

    async fn remove_server(
        &self,
        identity: &PluginIdentity,
        plugin_id: &str,
    ) -> Result<PluginResponse, ApiError> {
        let ids: Vec<String> = sqlx::query_scalar(
            "SELECT connection_id FROM zuse_plugin_connections WHERE tenant_id=$1 AND plugin_id=$2",
        )
        .bind(&identity.tenant)
        .bind(plugin_id)
        .fetch_all(&self.pool)
        .await?;
        for id in ids {
            self.remove_connection(&identity.tenant, &id).await?;
        }
        let at = now();
        let mut tx = self.pool.begin().await?;
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
            .bind(format!("plugins:{}", identity.tenant))
            .execute(&mut *tx)
            .await?;
        let removed =
            sqlx::query("DELETE FROM zuse_plugin_servers WHERE tenant_id=$1 AND server_id=$2")
                .bind(&identity.tenant)
                .bind(plugin_id)
                .execute(&mut *tx)
                .await?
                .rows_affected();
        if removed == 0 {
            return Err(invalid());
        }
        // Every connection to it, whoever made it; a missing definition never resolves tools.
        let ids: Vec<String> = sqlx::query_scalar("DELETE FROM zuse_plugin_connections WHERE tenant_id=$1 AND plugin_id=$2 RETURNING connection_id")
            .bind(&identity.tenant)
            .bind(plugin_id)
            .fetch_all(&mut *tx)
            .await?;
        sqlx::query("UPDATE zuse_plugin_attempts SET state='cancelled',error=NULL,authorization_url=NULL,state_digest=NULL,ticket_digest=NULL,updated_at=$3 WHERE tenant_id=$1 AND plugin_id=$2 AND state='pending'")
            .bind(&identity.tenant)
            .bind(plugin_id)
            .bind(at)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        let _ = ids;
        Ok(PluginResponse::Ok)
    }

    async fn poll(
        &self,
        identity: &PluginIdentity,
        attempt_id: &str,
        cancel: bool,
    ) -> Result<PluginResponse, ApiError> {
        let attempt = self
            .attempt(&identity.tenant, attempt_id)
            .await?
            .filter(|a| a.subject == identity.subject)
            .ok_or_else(failed)?;
        if attempt.state == "pending" && (cancel || attempt.expires_at <= now()) {
            self.fail(&identity.tenant, attempt_id, "cancelled", None)
                .await?;
            return self
                .attempt(&identity.tenant, attempt_id)
                .await?
                .map(|a| a.public())
                .ok_or_else(failed);
        }
        Ok(attempt.public())
    }

    /// Redeems the one-use ticket from the provider callback. Only the subject
    /// that started the attempt can complete it.
    async fn complete(
        &self,
        identity: &PluginIdentity,
        ticket: &str,
    ) -> Result<PluginResponse, ApiError> {
        if ticket.is_empty() || ticket.len() > 256 {
            return Err(invalid());
        }
        let ticket_digest = digest(ticket);
        let row = sqlx::query(
            "SELECT * FROM zuse_plugin_attempts WHERE tenant_id=$1 AND ticket_digest=$2",
        )
        .bind(&identity.tenant)
        .bind(&ticket_digest)
        .fetch_optional(&self.pool)
        .await?;
        let attempt = row
            .as_ref()
            .map(Attempt::from_row)
            .filter(|a| a.subject == identity.subject && a.state == "pending")
            .ok_or_else(|| ApiError::bad_request("invalid_plugin_confirmation"))?;
        if attempt.expires_at <= now() {
            self.fail(&attempt.tenant, &attempt.id, "cancelled", None)
                .await?;
            return self
                .attempt(&attempt.tenant, &attempt.id)
                .await?
                .map(|a| a.public())
                .ok_or_else(failed);
        }
        // Claim the ticket so a replay cannot exchange the code twice.
        let claimed = sqlx::query("UPDATE zuse_plugin_attempts SET ticket_digest=NULL,updated_at=$4 WHERE tenant_id=$1 AND attempt_id=$2 AND ticket_digest=$3 AND state='pending'")
            .bind(&attempt.tenant)
            .bind(&attempt.id)
            .bind(&ticket_digest)
            .bind(now())
            .execute(&self.pool)
            .await?
            .rows_affected();
        if claimed != 1 || attempt.ticket_digest.as_deref() != Some(ticket_digest.as_str()) {
            return Err(ApiError::bad_request("invalid_plugin_confirmation"));
        }
        let secret: AttemptSecret =
            self.open("attempt", &attempt.tenant, &attempt.id, &attempt.sealed)?;
        let query = secret.query.as_deref().ok_or_else(invalid)?;
        let row = self.visible_row(identity, &attempt.id).await?;
        let definition = self
            .definition(&identity.tenant, &row.plugin_id)
            .await?
            .ok_or_else(failed)?;
        let reference = self.reference(&identity.tenant, &row, &definition)?;
        let outcome=async {
            let reference=self.engine(json!({"operation":"complete","owner":identity.tenant,"ref":reference,"definition":definition,"callbackUrl":format!("{}?{}",self.callback_url(),query)})).await?;
            self.save_ref(&identity.tenant,&attempt.id,&definition,Auth::Oauth,&reference).await?;
            self.finish(&identity.tenant,&attempt.id,Auth::Oauth,&secret.label,secret.return_to.clone()).await
        }.await.map_err(|_|PluginFailure::SignInFailed);
        if let Err(failure) = outcome {
            tracing::warn!(plugin = %attempt.plugin_id, code = failure.code(), "Plugin sign-in failed");
            self.fail(
                &attempt.tenant,
                &attempt.id,
                "failed",
                Some(PluginFailure::SignInFailed),
            )
            .await?;
        }
        self.attempt(&attempt.tenant, &attempt.id)
            .await?
            .map(|a| a.public())
            .ok_or_else(failed)
    }

    /// Provider redirect target. Stores the callback and hands an opaque ticket
    /// to the app (desktop loopback or hosted web) that started the attempt.
    pub async fn callback(self: &Arc<Self>, raw_query: &str) -> Result<Url, ApiError> {
        if raw_query.len() > 16384 {
            return Err(invalid());
        }
        let control = self.clone();
        let query = raw_query.to_owned();
        tokio::spawn(async move { control.callback_inner(&query).await })
            .await
            .map_err(|_| ApiError::unavailable("plugin_operation_interrupted"))?
    }
    async fn callback_inner(&self, raw_query: &str) -> Result<Url, ApiError> {
        if raw_query.len() > 16_384 {
            return Err(invalid());
        }
        let params: Vec<(String, String)> = url::form_urlencoded::parse(raw_query.as_bytes())
            .into_owned()
            .collect();
        let param = |name: &str| {
            params
                .iter()
                .find(|(k, _)| k == name)
                .map(|(_, v)| v.as_str())
        };
        let state = param("state")
            .filter(|s| !s.is_empty() && s.len() <= 4096)
            .ok_or_else(|| ApiError::bad_request("invalid_plugin_callback"))?;
        let row = sqlx::query("SELECT * FROM zuse_plugin_attempts WHERE state_digest=$1")
            .bind(digest(state))
            .fetch_optional(&self.pool)
            .await?;
        let attempt = row
            .as_ref()
            .map(Attempt::from_row)
            .filter(|a| a.state == "pending" && a.ticket_digest.is_none() && a.expires_at > now())
            .ok_or_else(|| ApiError::bad_request("invalid_plugin_callback"))?;
        let _guard = self.admit(&attempt.tenant).await?;
        let _changes = self.views.changes(&attempt.tenant);
        let attempt = self
            .attempt(&attempt.tenant, &attempt.id)
            .await?
            .filter(|a| a.state == "pending" && a.ticket_digest.is_none() && a.expires_at > now())
            .ok_or_else(invalid)?;
        let mut secret: AttemptSecret =
            self.open("attempt", &attempt.tenant, &attempt.id, &attempt.sealed)?;
        let mut target = self.return_target(secret.return_to.as_ref())?;
        let name = self
            .definition(&attempt.tenant, &attempt.plugin_id)
            .await?
            .map(|d| d.name)
            .unwrap_or_else(|| attempt.plugin_id.clone());
        if param("error").is_some() {
            self.fail(&attempt.tenant, &attempt.id, "cancelled", None)
                .await?;
            target
                .query_pairs_mut()
                .append_pair("plugin_error", "cancelled")
                .append_pair("plugin", &name);
            return Ok(target);
        }
        if param("code").is_none_or(|c| c.is_empty() || c.len() > 8192) {
            return Err(ApiError::bad_request("invalid_plugin_callback"));
        }
        let ticket = random_token();
        secret.query = Some(raw_query.to_owned());
        let sealed = self.seal("attempt", &attempt.tenant, &attempt.id, &secret)?;
        let updated = sqlx::query("UPDATE zuse_plugin_attempts SET sealed=$3,ticket_digest=$4,state_digest=NULL,updated_at=$5 WHERE tenant_id=$1 AND attempt_id=$2 AND state='pending' AND ticket_digest IS NULL AND state_digest=$6")
            .bind(&attempt.tenant)
            .bind(&attempt.id)
            .bind(sealed)
            .bind(digest(&ticket))
            .bind(now())
            .bind(digest(state))
            .execute(&self.pool)
            .await?
            .rows_affected();
        if updated != 1 {
            return Err(ApiError::bad_request("invalid_plugin_callback"));
        }
        target
            .query_pairs_mut()
            .append_pair("plugin_ticket", &ticket)
            .append_pair("plugin_tenant", &attempt.tenant)
            .append_pair("plugin", &name);
        Ok(target)
    }

    async fn tool_list(
        &self,
        tenant: &str,
        row: &ConnectionRow,
        definition: &Definition,
    ) -> Result<Vec<Value>, ApiError> {
        let reference = self.reference(tenant, row, definition)?;
        let value = self
            .engine(
                json!({"operation":"list","owner":tenant,"ref":reference,"definition":definition}),
            )
            .await?;
        serde_json::from_value(value).map_err(|_| failed())
    }

    /// Discovery and invocation for agents. The tenant comes from the verified
    /// identity; the caller has already applied session permission policy.
    pub async fn tools(
        self: &Arc<Self>,
        identity: &PluginIdentity,
        input: PluginToolRequest,
    ) -> Result<Value, ApiError> {
        let control = self.clone();
        let identity = identity.clone();
        tokio::spawn(async move { control.tools_inner(&identity, input).await })
            .await
            .map_err(|_| ApiError::unavailable("plugin_operation_interrupted"))?
    }
    async fn tools_inner(
        &self,
        identity: &PluginIdentity,
        input: PluginToolRequest,
    ) -> Result<Value, ApiError> {
        let _guard = self.admit_local(&identity.tenant).await?;
        let (view, visible) = self.visible_cached(identity).await?;
        let mut connected = Vec::new();
        for row in visible {
            if row.state != "connected" || !row.enabled {
                continue;
            }
            if let Some(definition) = Self::view_definition(&view, &row.plugin_id) {
                connected.push((row, definition));
            }
        }
        match &input {
            PluginToolRequest::List => Ok(Value::Array(
                connected
                    .iter()
                    .map(|(row, _)| json!({"connectionId": row.id, "pluginId": row.plugin_id, "label": row.label}))
                    .collect(),
            )),
            PluginToolRequest::Search { query } => {
                Ok(Value::Array(self.search(identity, &connected, &query).await?))
            }
            PluginToolRequest::Schema { address } | PluginToolRequest::Call { address, .. } => {
                let (row,definition,tool)=Self::target(&connected,address)?;
                let reference=self.reference(&identity.tenant,row,definition)?;
                let (operation,arguments)=match &input {PluginToolRequest::Call{arguments,..}=>("call",Value::Object(arguments.clone())),_=>("schema",Value::Null)};
                let result=self.engine(json!({"operation":operation,"owner":identity.tenant,"ref":reference,"definition":definition,"tool":tool,"arguments":arguments})).await;
                if result.as_ref().is_err_and(|error|error.code=="plugin_needs_auth") {
                    let _changes = self.views.changes(&identity.tenant);
                    sqlx::query("UPDATE zuse_plugin_connections SET state='needs-auth',updated_at=$3 WHERE tenant_id=$1 AND connection_id=$2").bind(&identity.tenant).bind(&row.id).bind(now()).execute(&self.pool).await?;
                }
                result
            }
        }
    }

    fn target<'a>(
        connected: &'a [(ConnectionRow, Definition)],
        address: &'a str,
    ) -> Result<(&'a ConnectionRow, &'a Definition, &'a str), ApiError> {
        connected
            .iter()
            .find_map(|(row, definition)| {
                address
                    .strip_prefix(&row.prefix())
                    .filter(|tool| !tool.is_empty())
                    .map(|tool| (row, definition, tool))
            })
            .ok_or_else(failed)
    }

    /// Discovers concurrently under one deadline so a stalled connection cannot
    /// hide other plugins' tools; results are consumed in connection order.
    async fn search(
        &self,
        identity: &PluginIdentity,
        connected: &[(ConnectionRow, Definition)],
        query: &str,
    ) -> Result<Vec<Value>, ApiError> {
        let terms: Vec<String> = query
            .chars()
            .take(200)
            .collect::<String>()
            .to_lowercase()
            .split_whitespace()
            .map(str::to_owned)
            .collect();
        let deadline = tokio::time::Instant::now() + SEARCH_DEADLINE;
        use futures::FutureExt;
        use futures::StreamExt;
        let jobs: Vec<_> = connected
            .iter()
            .map(|(row, definition)| {
                tokio::time::timeout_at(deadline, self.tool_list(&identity.tenant, row, definition))
                    .boxed()
            })
            .collect();
        let mut lists = futures::stream::iter(jobs).buffered(2);
        let alphanumeric = |s: &str| {
            s.chars()
                .filter(|c| c.is_ascii_alphanumeric())
                .collect::<String>()
        };
        let mut tools = Vec::new();
        let mut failure = None;
        let mut reachable = false;
        for (row, definition) in connected.iter() {
            let Some(list) = lists.next().await else {
                break;
            };
            let list = match list {
                Ok(Ok(list)) => list,
                Ok(Err(error)) => {
                    tracing::warn!(plugin = %row.plugin_id, code = error.code, "Plugin tools unavailable");
                    failure.get_or_insert(error);
                    continue;
                }
                Err(_) => {
                    failure.get_or_insert(PluginFailure::Unreachable.into());
                    continue;
                }
            };
            reachable = true;
            let plugin = format!("{} {} {}", row.plugin_id, definition.name, row.label);
            let service = alphanumeric(&plugin.to_lowercase());
            for tool in &list {
                let name = tool["name"].as_str().ok_or_else(failed)?;
                let description = tool["description"].as_str().unwrap_or(name);
                let haystack = format!("{plugin} {name} {description}").to_lowercase();
                if terms.iter().all(|term| {
                    let compact = alphanumeric(term);
                    haystack.contains(term.as_str())
                        || (!compact.is_empty() && service.contains(&compact))
                }) {
                    tools.push(json!({"address": row.prefix() + name, "description": description}));
                }
                if tools.len() >= 100 {
                    return Ok(tools);
                }
            }
        }
        match failure {
            Some(failure) if !reachable => Err(failure),
            _ => Ok(tools),
        }
    }

    /// Cancels expired attempts and purges terminal ones. Safe to run on every replica.
    pub async fn sweep(&self) -> Result<u64, ApiError> {
        let at = now();
        let expired: Vec<(String, String)> = sqlx::query_as("SELECT tenant_id,attempt_id FROM zuse_plugin_attempts WHERE state='pending' AND expires_at<=$1 ORDER BY expires_at LIMIT 100")
            .bind(at)
            .fetch_all(&self.pool)
            .await?;
        for (tenant, id) in &expired {
            let _guard = self.admit(tenant).await?;
            let _changes = self.views.changes(tenant);
            self.fail(tenant, id, "cancelled", None).await?;
        }
        let purged = sqlx::query(
            "DELETE FROM zuse_plugin_attempts WHERE state<>'pending' AND expires_at<$1",
        )
        .bind(at - TERMINAL_RETENTION_MS)
        .execute(&self.pool)
        .await?
        .rows_affected();
        Ok(expired.len() as u64 + purged)
    }

    pub async fn serve_mcp(
        self: &Arc<Self>,
        identity: &PluginIdentity,
        message: Value,
        headers: Value,
    ) -> Result<Value, ApiError> {
        let prepared = self
            .engine(json!({"operation":"mcp","message":message,"headers":headers}))
            .await?;
        let Some(command) = prepared.get("command") else {
            return Ok(prepared);
        };
        let request: PluginToolRequest =
            serde_json::from_value(command.clone()).map_err(|_| invalid())?;
        let result = self.tools(identity, request).await;
        self.engine(json!({"operation":"mcp","message":message,"headers":headers,"result":result.as_ref().ok(),"failed":result.is_err()})).await
    }
}

fn random_token() -> String {
    use aes_gcm::aead::{OsRng, rand_core::RngCore};
    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_ids_and_headers_follow_the_hosted_contract() {
        assert!(valid_uuid("123e4567-e89b-12d3-a456-426614174000"));
        assert!(!valid_uuid("123E4567-E89B-12D3-A456-426614174000"));
        assert!(!valid_uuid("123e4567e89b12d3a456426614174000"));
        assert!(valid_header_name("X-Api-Key"));
        assert!(!valid_header_name("Content-Type"));
        assert!(!valid_header_name("bad header"));
        assert!(!valid_key("a\r\nb"));
    }

    #[test]
    fn custom_servers_never_expose_query_strings() {
        let public = public_server(
            "custom-0",
            &CustomServer {
                name: "Tools".into(),
                endpoint: "https://mcp.tools.example/v1?token=secret".into(),
                header: None,
            },
        )
        .unwrap();
        assert_eq!(public.url.as_deref(), Some("https://mcp.tools.example/v1"));
        assert_eq!(public.domain, "tools.example");
        assert!(public.custom);
    }
}

impl crate::store::Store {
    pub fn with_plugin_control(mut self, control: Arc<PluginControl>) -> Self {
        self.plugins = Some(control);
        self
    }
}
