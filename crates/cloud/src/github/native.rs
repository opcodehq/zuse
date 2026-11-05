//! Opt-in standalone authority. A failed native authorization never falls back to
//! an installation administrator, a process token, or the transitional Worker.
use crate::{access::Access, crypto::MasterKey, error::ApiError};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Row};
use std::{collections::BTreeSet, sync::Arc, time::Duration};
use zeroize::Zeroizing;
use zuse_protocol::cloud_projects::{
    GithubCatalog, GithubInstallation, GithubRepository, GithubUser,
};

// GitHub's documented delivery cap and the original Worker receiver boundary.
pub(crate) const GITHUB_WEBHOOK_BODY_LIMIT: usize = 25 * 1024 * 1024;

mod browser;
mod enrollment;
mod installations;
mod oauth;
pub mod organizations;
pub use browser::{BrowserSelection, BrowserTransaction};
pub use oauth::{Callback, Selection};
#[cfg(test)]
pub(crate) mod admission_fixture;
#[cfg(test)]
mod catalog_unlink_tests;
mod repository_access;
mod tests;
pub(crate) use repository_access::{RepositoryAuthorization, mark_project_access};

type ObservationGates = tokio::sync::Mutex<
    std::collections::HashMap<(String, i64), std::sync::Weak<tokio::sync::Mutex<()>>>,
>;
pub struct NativeGithub {
    membership_epoch: std::sync::atomic::AtomicU64,
    observation_gates: ObservationGates,
    fast_start_accounts: BTreeSet<String>,
    pool: PgPool,
    master: MasterKey,
    accounts: BTreeSet<String>,
    config: Config,
    http: reqwest::Client,
}
struct Config {
    origin: reqwest::Url,
    app_id: String,
    slug: String,
    client_id: String,
    client_secret: Zeroizing<String>,
    signing_key: jsonwebtoken::EncodingKey,
    webhook_secret: Zeroizing<String>,
    api_origin: reqwest::Url,
    oauth_origin: reqwest::Url,
}
#[derive(Serialize, Deserialize)]
struct Credentials {
    access_token: String,
    expires_at: Option<i64>,
    refresh_token: Option<String>,
    refresh_expires_at: Option<i64>,
}
impl Drop for Credentials {
    fn drop(&mut self) {
        use zeroize::Zeroize;
        self.access_token.zeroize();
        if let Some(value) = &mut self.refresh_token {
            value.zeroize();
        }
    }
}
#[derive(Serialize, Deserialize)]
struct Pending {
    credentials: Credentials,
    profile: Value,
    #[serde(default)]
    browser_installation: Option<i64>,
}
/// Plaintext is deliberately neither Debug nor Serialize. Runtime callers must
/// seal the token to their generation-bound recipient before returning it.
pub struct ScopedGitCredential {
    pub github_user_id: i64,
    pub token: Zeroizing<String>,
    /// Local authority lease; does not change the GitHub bearer lifetime.
    pub authority_expires_at: i64,
    pub provider_expires_at: i64,
    pub login: String,
    pub email: String,
}
fn now() -> i64 {
    chrono::Utc::now().timestamp_millis()
}
fn unavailable() -> ApiError {
    ApiError::unavailable("cloud_github_unavailable")
}
fn reconnect() -> ApiError {
    ApiError::conflict("github_reconnect_required")
}
fn hash(value: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(value.as_bytes()))
}
fn opaque() -> String {
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}
fn valid_component(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 100
        && value != "."
        && value != ".."
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
}
fn can_manage(access: &Access) -> bool {
    access
        .membership
        .as_ref()
        .is_none_or(|member| member.role.slug == "admin")
}
impl NativeGithub {
    /// Empty rollout is a no-op; credentials are not read until explicitly enabled.
    pub fn from_env(pool: PgPool) -> anyhow::Result<Option<Arc<Self>>> {
        let accounts: BTreeSet<_> = std::env::var("ZUSE_CLOUD_GITHUB_WRITE_ACCOUNTS")
            .unwrap_or_default()
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .collect();
        if accounts.is_empty() {
            return Ok(None);
        }
        fn required(name: &str) -> anyhow::Result<String> {
            let value = if let Ok(path) = std::env::var(format!("{name}_FILE")) {
                anyhow::ensure!(
                    std::path::Path::new(&path).is_absolute(),
                    "Secret file path must be absolute"
                );
                std::fs::read_to_string(path)
                    .map_err(|_| anyhow::anyhow!("Cannot read {name} file"))?
                    .trim_end_matches(['\r', '\n'])
                    .to_owned()
            } else {
                std::env::var(name).map_err(|_| anyhow::anyhow!("Missing {name}"))?
            };
            anyhow::ensure!(!value.trim().is_empty(), "Missing {name}");
            Ok(value)
        }
        let origin = reqwest::Url::parse(&required("ZUSE_CLOUD_GITHUB_ORIGIN")?)?;
        anyhow::ensure!(
            origin.scheme() == "https"
                && origin.host_str().is_some()
                && origin.username().is_empty()
                && origin.password().is_none()
                && origin.query().is_none()
                && origin.fragment().is_none()
                && origin.path() == "/",
            "GitHub origin must be an HTTPS origin"
        );
        let key = if let Ok(path) = std::env::var("GITHUB_APP_PRIVATE_KEY_FILE") {
            anyhow::ensure!(
                std::path::Path::new(&path).is_absolute(),
                "GitHub key path must be absolute"
            );
            Zeroizing::new(
                std::fs::read(path)
                    .map_err(|_| anyhow::anyhow!("Cannot read GitHub signing key"))?,
            )
        } else {
            Zeroizing::new(required("GITHUB_APP_PRIVATE_KEY")?.into_bytes())
        };
        let signing_key = signing_key(&key)?;
        let app_id = required("GITHUB_APP_ID")?;
        anyhow::ensure!(
            app_id.parse::<u64>().is_ok_and(|n| n > 0),
            "Invalid GitHub App ID"
        );
        let slug = required("GITHUB_APP_SLUG")?;
        anyhow::ensure!(valid_component(&slug), "Invalid GitHub App slug");
        let master = MasterKey::parse(&required("CLOUD_DATA_ENCRYPTION_KEY")?)
            .map_err(|_| anyhow::anyhow!("Invalid GitHub encryption key"))?;
        Ok(Some(Arc::new(Self {
            membership_epoch: std::sync::atomic::AtomicU64::new(0),
            observation_gates: Default::default(),
            fast_start_accounts: if std::env::var("ZUSE_DEPLOYMENT_ENVIRONMENT").as_deref() == Ok("staging") {
                std::env::var("ZUSE_CLOUD_FAST_START_ACCOUNTS").unwrap_or_default().split(',').map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned).collect()
            } else { BTreeSet::new() },
            pool,
            master,
            accounts,
            config: Config {
                origin,
                app_id,
                slug,
                client_id: required("GITHUB_APP_CLIENT_ID")?,
                client_secret: Zeroizing::new(required("GITHUB_APP_CLIENT_SECRET")?),
                signing_key,
                webhook_secret: Zeroizing::new(required("GITHUB_WEBHOOK_SECRET")?),
                api_origin: reqwest::Url::parse("https://api.github.com/")?,
                oauth_origin: reqwest::Url::parse("https://github.com/")?,
            },
            http: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(Duration::from_secs(20))
                .build()?,
        })))
    }
    pub async fn connected_user_id(&self, actor: &str) -> Result<i64, ApiError> {
        sqlx::query_scalar("SELECT github_user_id FROM zuse_rust_github_users WHERE actor_id=$1 AND state='connected' AND NOT refresh_uncertain").bind(actor).fetch_optional(&self.pool).await?.flatten().filter(|id|*id>0).ok_or_else(reconnect)
    }
    pub async fn connection_state(&self, access: &Access) -> Result<String, ApiError> {
        self.require(access)?;
        Ok(
            sqlx::query_scalar("SELECT state FROM zuse_rust_github_users WHERE actor_id=$1")
                .bind(&access.actor.account_id)
                .fetch_optional(&self.pool)
                .await?
                .unwrap_or_else(|| "disconnected".into()),
        )
    }
    pub async fn ready(&self) -> Result<(), ApiError> {
        let read_only: bool =
            sqlx::query_scalar("SELECT current_setting('default_transaction_read_only')::boolean")
                .fetch_one(&self.pool)
                .await?;
        if read_only {
            return Err(ApiError::unavailable("github_native_requires_write_pool"));
        }
        let ready:bool=sqlx::query_scalar("SELECT to_regclass('zuse_rust_github_users') IS NOT NULL AND to_regclass('zuse_rust_github_installations') IS NOT NULL AND to_regclass('zuse_rust_github_operations') IS NOT NULL AND to_regclass('zuse_rust_github_deliveries') IS NOT NULL").fetch_one(&self.pool).await?;
        if !ready {
            return Err(ApiError::unavailable("github_native_schema_not_ready"));
        }
        Ok(())
    }
    pub(crate) fn shared_pool(&self) -> PgPool {
        self.pool.clone()
    }
    pub fn membership_revision(&self) -> u64 {
        self.membership_epoch
            .load(std::sync::atomic::Ordering::Acquire)
    }
    pub fn enabled(&self, owner: &str) -> bool {
        self.accounts.contains(owner)
    }
    fn require(&self, access: &Access) -> Result<(), ApiError> {
        if !self.enabled(&access.owner_id) {
            return Err(ApiError::forbidden("github_native_disabled"));
        }
        Ok(())
    }
    async fn response(&self, request: reqwest::RequestBuilder) -> Result<(u16, Value), ApiError> {
        let mut response = request
            .header("Accept", "application/vnd.github+json")
            .header("User-Agent", "Zuse-GitHub-Authority")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .send()
            .await
            .map_err(|_| unavailable())?;
        let status = response.status().as_u16();
        let mut bytes = Zeroizing::new(Vec::new());
        while let Some(chunk) = response.chunk().await.map_err(|_| unavailable())? {
            if bytes.len() + chunk.len() > 8 * 1024 * 1024 {
                return Err(unavailable());
            }
            bytes.extend_from_slice(&chunk);
        }
        let value = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).map_err(|_| unavailable())?
        };
        Ok((status, value))
    }
    fn api(&self, path: &str) -> reqwest::Url {
        // Paths are constructed only from verified numeric IDs or ASCII components.
        self.config
            .api_origin
            .join(path.trim_start_matches('/'))
            .expect("fixed GitHub API path")
    }
    async fn get(&self, path: &str, token: &str) -> Result<Value, ApiError> {
        let (status, value) = self
            .response(self.http.get(self.api(path)).bearer_auth(token))
            .await?;
        match status {
            200 => Ok(value),
            401 => Err(reconnect()),
            403 | 404 => Err(ApiError::forbidden("github_repository_access_denied")),
            _ => Err(unavailable()),
        }
    }
    fn app_token(&self) -> Result<String, ApiError> {
        let seconds = now() / 1000;
        jsonwebtoken::encode(
            &jsonwebtoken::Header::new(jsonwebtoken::Algorithm::RS256),
            &json!({"iat":seconds-60,"exp":seconds+540,"iss":self.config.app_id}),
            &self.config.signing_key,
        )
        .map_err(|_| unavailable())
    }
    fn seal<T: Serialize>(&self, context: &str, value: &T) -> Result<String, ApiError> {
        let raw = Zeroizing::new(serde_json::to_string(value).map_err(|_| unavailable())?);
        self.master
            .seal_api(context, &raw)
            .map_err(|_| unavailable())
    }
    fn open<T: serde::de::DeserializeOwned>(
        &self,
        context: &str,
        value: &str,
    ) -> Result<T, ApiError> {
        let raw = self
            .master
            .open_api(context, value)
            .map_err(|_| unavailable())?;
        serde_json::from_slice(&raw).map_err(|_| unavailable())
    }
    /// A database advisory transaction serializes rotations across API processes.
    /// Rotation is written through the pool before follow-on repository requests.
    #[tracing::instrument(name="cloud_admission_stage", skip_all, fields(actor=%actor, operation="github_credentials"))]
    async fn credentials(&self, actor: &str) -> Result<Credentials, ApiError> {
        // Coalesce simultaneous workspace requests behind an in-flight rotation.
        // A crashed/lost refresh is bounded and ultimately requires reconnect.
        for _ in 0..40 {
            match self.credentials_inner(actor, false).await {
                Err(error) if error.code == "github_refresh_uncertain" => {
                    tokio::time::sleep(Duration::from_millis(50)).await
                }
                result => return result,
            }
        }
        Err(ApiError::conflict("github_refresh_uncertain"))
    }
    async fn credentials_inner(
        &self,
        actor: &str,
        allow_revoking: bool,
    ) -> Result<Credentials, ApiError> {
        let mut lock = self.pool.begin().await?;
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
            .bind(format!("github-user:{actor}"))
            .execute(&mut *lock)
            .await?;
        let row=sqlx::query("SELECT sealed_credentials,epoch,version,refresh_uncertain,state FROM zuse_rust_github_users WHERE actor_id=$1 AND (state='connected' OR ($2 AND state='revoking'))")
            .bind(actor).bind(allow_revoking).fetch_optional(&mut *lock).await?.ok_or_else(reconnect)?;
        if row.try_get::<bool, _>("refresh_uncertain")? {
            return Err(ApiError::conflict("github_refresh_uncertain"));
        }
        let sealed: String = row.try_get("sealed_credentials")?;
        let epoch: i64 = row.try_get("epoch")?;
        let version: i64 = row.try_get("version")?;
        let state: String = row.try_get("state")?;
        let current: Credentials = self.open(&format!("github-user:{actor}:{epoch}"), &sealed)?;
        if current
            .expires_at
            .is_none_or(|expiry| expiry > now() + 60_000)
        {
            lock.commit().await?;
            return Ok(current);
        }
        let refresh = current
            .refresh_token
            .as_deref()
            .filter(|_| {
                current
                    .refresh_expires_at
                    .is_some_and(|expiry| expiry > now())
            })
            .ok_or_else(reconnect)?;
        // A lost refresh response cannot safely be replayed with a rotating token.
        // Persist uncertainty before HTTP; only the matching response clears it.
        sqlx::query("UPDATE zuse_rust_github_users SET refresh_uncertain=true WHERE actor_id=$1 AND epoch=$2 AND version=$3")
            .bind(actor).bind(epoch).bind(version).execute(&mut *lock).await?;
        lock.commit().await?;
        let next = self
            .exchange(&[("grant_type", "refresh_token"), ("refresh_token", refresh)])
            .await?;
        let sealed = self.seal(&format!("github-user:{actor}:{epoch}"), &next)?;
        let updated=sqlx::query("UPDATE zuse_rust_github_users SET sealed_credentials=$4,version=version+1,updated_at=$5,refresh_uncertain=false WHERE actor_id=$1 AND epoch=$2 AND version=$3 AND state=$6 AND refresh_uncertain")
            .bind(actor).bind(epoch).bind(version).bind(sealed).bind(now()).bind(state).execute(&self.pool).await?;
        if updated.rows_affected() != 1 {
            return Err(reconnect());
        }
        Ok(next)
    }
    async fn exchange(&self, params: &[(&str, &str)]) -> Result<Credentials, ApiError> {
        let mut fields = vec![
            ("client_id", self.config.client_id.as_str()),
            ("client_secret", self.config.client_secret.as_str()),
        ];
        fields.extend_from_slice(params);
        let (status, value) = self
            .response(
                self.http
                    .post(
                        self.config
                            .oauth_origin
                            .join("login/oauth/access_token")
                            .map_err(|_| unavailable())?,
                    )
                    .form(&fields),
            )
            .await?;
        if status != 200 || value.get("error").is_some() {
            return Err(reconnect());
        }
        parse_credentials(&value, now())
    }
    async fn repositories(&self, installation: i64, token: &str) -> Result<Vec<Value>, ApiError> {
        let mut result = Vec::new();
        // Construct each page ourselves. Never follow a token-bearing foreign Link.
        for page in 1..=100 {
            let value = self
                .get(
                    &format!(
                        "/user/installations/{installation}/repositories?per_page=100&page={page}"
                    ),
                    token,
                )
                .await?;
            let entries = value["repositories"].as_array().ok_or_else(unavailable)?;
            result.extend(entries.iter().cloned());
            if entries.len() < 100 {
                return Ok(result);
            }
        }
        Err(ApiError::unavailable("github_pagination_limit"))
    }
    async fn installation(&self, id: i64) -> Result<Option<Value>, ApiError> {
        let (status, value) = self
            .response(
                self.http
                    .get(self.api(&format!("/app/installations/{id}")))
                    .bearer_auth(self.app_token()?),
            )
            .await?;
        match status {
            200 => Ok(Some(value)),
            404 => Ok(None),
            _ => Err(unavailable()),
        }
    }
    /// Observation fencing prevents a stale webhook/refresh from undoing a newer
    /// refresh, disconnect, or explicit removal. Inspection never creates links.
    async fn observe(&self, owner: &str, id: i64) -> Result<Option<Value>, ApiError> {
        // Coordinate local observers before they replace the durable nonce.
        // Each caller still performs a fresh provider read and the existing
        // conditional write; removal and external supersession remain fences.
        // No database transaction or connection is held while waiting here.
        let gate = {
            let mut gates = self.observation_gates.lock().await;
            gates.retain(|_, value| value.strong_count() > 0);
            let key = (owner.to_owned(), id);
            if let Some(gate) = gates.get(&key).and_then(std::sync::Weak::upgrade) {
                gate
            } else {
                let gate = Arc::new(tokio::sync::Mutex::new(()));
                gates.insert(key, Arc::downgrade(&gate));
                gate
            }
        };
        let _observation = gate.lock().await;
        let nonce = opaque();
        let rows=sqlx::query("UPDATE zuse_rust_github_installations SET observation_id=$3 WHERE owner_id=$1 AND installation_id=$2 AND state<>'removed'")
            .bind(owner).bind(id).bind(&nonce).execute(&self.pool).await?;
        if rows.rows_affected() == 0 {
            return Ok(None);
        }
        let metadata = self.installation(id).await?;
        let state = match &metadata {
            None => "removed",
            Some(v) if !v["suspended_at"].is_null() => "suspended",
            Some(_) => "active",
        };
        let result=sqlx::query("UPDATE zuse_rust_github_installations SET metadata=COALESCE($4,metadata),state=$5,revision=revision+1,updated_at=$6 WHERE owner_id=$1 AND installation_id=$2 AND observation_id=$3 AND state<>'removed'")
            .bind(owner).bind(id).bind(&nonce).bind(metadata.as_ref()).bind(state).bind(now()).execute(&self.pool).await?;
        if result.rows_affected() != 1 {
            return Err(ApiError::conflict("github_observation_superseded"));
        }
        Ok(metadata.filter(|_| state == "active"))
    }
    pub async fn catalog(&self, access: &Access) -> Result<GithubCatalog, ApiError> {
        self.require(access)?;
        let mut result = GithubCatalog {
            configured: true,
            can_manage_installations: Some(can_manage(access)),
            user: None,
            installations: vec![],
            repositories: vec![],
        };
        let profile: Option<Value> = sqlx::query_scalar(
            "SELECT profile FROM zuse_rust_github_users WHERE actor_id=$1 AND state='connected'",
        )
        .bind(&access.actor.account_id)
        .fetch_optional(&self.pool)
        .await?;
        let Some(profile) = profile else {
            return Ok(result);
        };
        result.user = Some(public_user(&profile)?);
        let credentials = self.credentials(&access.actor.account_id).await?;
        let ids:Vec<i64>=sqlx::query_scalar("SELECT installation_id FROM zuse_rust_github_installations WHERE owner_id=$1 AND state<>'removed' ORDER BY installation_id").bind(&access.owner_id).fetch_all(&self.pool).await?;
        for id in ids {
            let Some(metadata) = self.observe(&access.owner_id, id).await? else {
                continue;
            };
            let repos = self.repositories(id, &credentials.access_token).await?;
            let current:Option<Option<Value>>=sqlx::query_scalar("SELECT allowed_repositories FROM zuse_rust_github_installations WHERE owner_id=$1 AND installation_id=$2 AND state='active'").bind(&access.owner_id).bind(id).fetch_optional(&self.pool).await?;
            // SQL NULL means an unrestricted active link. A missing row means
            // authorization changed while the repository request was in flight.
            let Some(allowed) = current else { continue };
            result
                .installations
                .push(installation_summary(id, &metadata)?);
            for repo in repos {
                if repository_allowed(&metadata, allowed.as_ref(), &repo) {
                    result.repositories.push(repository_summary(&repo)?);
                }
            }
        }
        result
            .repositories
            .sort_by(|a, b| a.name_with_owner.cmp(&b.name_with_owner));
        result
            .repositories
            .dedup_by(|a, b| a.name_with_owner.eq_ignore_ascii_case(&b.name_with_owner));
        Ok(result)
    }
    /// Scope a user's authorization at GitHub before delivering a guest credential.
    /// An installation token is never a substitute for the initiating user's token.
    pub async fn scoped_git(
        &self,
        owner: &str,
        actor: &str,
        repository: &str,
    ) -> Result<ScopedGitCredential, ApiError> {
        if !self.enabled(owner) {
            return Err(ApiError::forbidden("github_native_disabled"));
        }
        let (target, name) = repository
            .split_once('/')
            .filter(|(a, b)| valid_component(a) && valid_component(b))
            .ok_or_else(|| ApiError::bad_request("invalid_github_repository"))?;
        // Independent reads: the actor credential and the owner's repository links.
        let (credentials, links) = tokio::join!(
            self.credentials(actor),
            self.repository_links(owner, repository),
        );
        let credentials = credentials?;
        let (scope_epoch, links) = links?;
        let before = self
            .credential_binding(actor, &credentials.access_token)
            .await?;
        // Both calls use the selected user's current credential. GitHub narrows
        // the token independently; it remains private until BOTH operations and
        // the fresh local revocation/binding checks below succeed.
        let ((installation, scope_epoch), value) = if self.fast_start_accounts.contains(owner) {
            let (installation, scoped) = tokio::join!(
                self.verify_repository_links(owner, repository, &credentials.access_token, scope_epoch, links),
                self.scoped_token_request(&credentials.access_token, target, name),
            );
            (installation?, scoped?)
        } else {
            let installation = self.verify_repository_links(owner, repository, &credentials.access_token, scope_epoch, links).await?;
            let scoped = self.scoped_token_request(&credentials.access_token, target, name).await?;
            (installation, scoped)
        };
        let token = value["token"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| ApiError::unavailable("github_scoped_token_missing"))?;
        if token == credentials.access_token {
            return Err(ApiError::unavailable("github_scoped_token_not_narrowed"));
        }
        let expiry = scoped_token_expiry(&value, credentials.expires_at, now())?;
        // Both post-checks start after the GitHub calls finished; run them together.
        let (after, still_allowed) = tokio::join!(
            self.credential_binding(actor, &credentials.access_token),
            sqlx::query_scalar::<_, bool>("SELECT EXISTS(SELECT 1 FROM zuse_rust_github_installations i JOIN zuse_rust_github_scopes s USING(owner_id) WHERE i.owner_id=$1 AND i.installation_id=$2 AND i.state='active' AND s.epoch=$3 AND lower(i.metadata#>>'{account,login}')=lower($4) AND CASE WHEN i.allowed_repositories IS NULL THEN true WHEN jsonb_typeof(i.allowed_repositories)='array' THEN EXISTS(SELECT 1 FROM jsonb_array_elements_text(i.allowed_repositories) AS allowed(name) WHERE lower(allowed.name)=lower($5)) ELSE false END)").bind(owner).bind(installation).bind(scope_epoch).bind(target).bind(repository).fetch_one(&self.pool),
        );
        let after = after?;
        if before != after {
            return Err(reconnect());
        }
        if !still_allowed? {
            return Err(ApiError::conflict("github_installation_changed"));
        }
        let profile = &after.2;
        let user = public_user(&profile)?;
        Ok(ScopedGitCredential {
            github_user_id: profile["id"]
                .as_i64()
                .filter(|id| *id > 0)
                .ok_or_else(unavailable)?,
            token: Zeroizing::new(token.to_owned()),
            authority_expires_at: expiry.authority_expires_at,
            provider_expires_at: expiry.provider_expires_at,
            login: user.login,
            email: user.email,
        })
    }
    #[tracing::instrument(name="cloud_admission_stage", skip_all, fields(operation="github_scoped_token"))]
    async fn scoped_token_request(&self, token: &str, target: &str, repository: &str) -> Result<Value, ApiError> {
        let (status, value) = self.response(
            self.http.post(self.api(&format!("/applications/{}/token/scoped", self.config.client_id)))
                .basic_auth(&self.config.client_id, Some(self.config.client_secret.as_str()))
                .json(&json!({"access_token":token,"target":target,"repositories":[repository]})),
        ).await?;
        if status != 200 && status != 201 {
            return Err(ApiError::forbidden("github_scoped_token_failed"));
        }
        Ok(value)
    }
    #[tracing::instrument(name="cloud_admission_stage", skip_all, fields(actor=%actor, operation="github_credential_binding"))]
    async fn credential_binding(
        &self,
        actor: &str,
        token: &str,
    ) -> Result<(i64, i64, Value), ApiError> {
        let row=sqlx::query("SELECT epoch,version,profile,sealed_credentials FROM zuse_rust_github_users WHERE actor_id=$1 AND state='connected' AND NOT refresh_uncertain").bind(actor).fetch_optional(&self.pool).await?.ok_or_else(reconnect)?;
        let epoch: i64 = row.try_get("epoch")?;
        let credentials: Credentials = self.open(
            &format!("github-user:{actor}:{epoch}"),
            &row.try_get::<String, _>("sealed_credentials")?,
        )?;
        if credentials.access_token != token {
            return Err(reconnect());
        }
        Ok((epoch, row.try_get("version")?, row.try_get("profile")?))
    }
    pub async fn verify_organization_member(
        &self,
        owner: &str,
        actor: &str,
        installation: i64,
        expected_user: i64,
        expected_org: i64,
    ) -> Result<(), ApiError> {
        if !self.enabled(owner) {
            return Err(ApiError::conflict("github_enrollment_migration_required"));
        }
        let credentials = self.credentials(actor).await?;
        let binding = self
            .credential_binding(actor, &credentials.access_token)
            .await?;
        if binding.2["id"] != expected_user {
            return Err(ApiError::forbidden("github_enrollment_identity_mismatch"));
        }
        let metadata = self
            .observe(owner, installation)
            .await?
            .ok_or_else(|| ApiError::forbidden("github_installation_unavailable"))?;
        if metadata["account"]["id"] != expected_org {
            return Err(ApiError::forbidden("github_enrollment_identity_mismatch"));
        }
        let login = metadata["account"]["login"]
            .as_str()
            .filter(|s| valid_component(s))
            .ok_or_else(unavailable)?;
        if metadata["account"]["type"] != "Organization" {
            return Err(ApiError::forbidden(
                "github_organization_membership_required",
            ));
        }
        let membership = self
            .get(
                &format!("/user/memberships/orgs/{login}"),
                &credentials.access_token,
            )
            .await?;
        if membership["state"] != "active"
            || !matches!(membership["role"].as_str(), Some("admin" | "member"))
        {
            return Err(ApiError::forbidden(
                "github_organization_membership_required",
            ));
        }
        Ok(())
    }
    pub async fn create_pull_request(
        &self,
        access: &Access,
        repository: &str,
        title: &str,
        head: &str,
        base: &str,
        body: &str,
        draft: bool,
    ) -> Result<Value, ApiError> {
        self.require(access)?;
        if title.trim().is_empty()
            || title.len() > 256
            || body.len() > 64 * 1024
            || !crate::runtime::creation::valid_git_ref(head)
            || !crate::runtime::creation::valid_git_ref(base)
        {
            return Err(ApiError::bad_request("invalid_github_pull_request"));
        }
        let grant = self
            .scoped_git(&access.owner_id, &access.actor.account_id, repository)
            .await?;
        let (status, value) = self
            .response(
                self.http
                    .post(self.api(&format!("/repos/{repository}/pulls")))
                    .bearer_auth(grant.token.as_str())
                    .json(
                        &json!({"title":title,"head":head,"base":base,"body":body,"draft":draft}),
                    ),
            )
            .await?;
        if status != 201 {
            return Err(ApiError::conflict("github_pull_request_failed"));
        }
        if draft && value["draft"] != true {
            return Err(ApiError::conflict(
                "github_pull_request_draft_not_confirmed",
            ));
        }
        let url = value["html_url"]
            .as_str()
            .and_then(|s| reqwest::Url::parse(s).ok())
            .filter(|url| {
                url.scheme() == "https"
                    && url.host_str() == Some("github.com")
                    && url.username().is_empty()
                    && url.password().is_none()
            })
            .ok_or_else(unavailable)?;
        Ok(
            json!({"url":url.as_str(),"number":value["number"].as_u64().ok_or_else(unavailable)?,"draft":value["draft"].as_bool().unwrap_or(false)}),
        )
    }
    pub async fn disconnect(&self, access: &Access) -> Result<(), ApiError> {
        self.require(access)?;
        let credentials = self
            .credentials_inner(&access.actor.account_id, true)
            .await?;
        drop(credentials);
        let mut tx = self.pool.begin().await?;
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
            .bind(format!("github-user:{}", access.actor.account_id))
            .execute(&mut *tx)
            .await?;
        let row=sqlx::query("SELECT epoch,version,sealed_credentials FROM zuse_rust_github_users WHERE actor_id=$1 AND state IN ('connected','revoking') AND NOT refresh_uncertain").bind(&access.actor.account_id).fetch_optional(&mut *tx).await?.ok_or_else(reconnect)?;
        let epoch: i64 = row.try_get("epoch")?;
        let version: i64 = row.try_get("version")?;
        let sealed: String = row.try_get("sealed_credentials")?;
        let current: Credentials = self.open(
            &format!("github-user:{}:{epoch}", access.actor.account_id),
            &sealed,
        )?;
        sqlx::query("UPDATE zuse_rust_github_users SET state='revoking',updated_at=$2 WHERE actor_id=$1 AND epoch=$3").bind(&access.actor.account_id).bind(now()).bind(epoch).execute(&mut *tx).await?;
        tx.commit().await?;
        let (status, _) = self
            .response(
                self.http
                    .delete(self.api(&format!("/applications/{}/grant", self.config.client_id)))
                    .basic_auth(
                        &self.config.client_id,
                        Some(self.config.client_secret.as_str()),
                    )
                    .json(&json!({"access_token":current.access_token})),
            )
            .await?;
        if status != 204 && status != 404 {
            return Err(unavailable());
        }
        let mut tx = self.pool.begin().await?;
        let removed=sqlx::query("UPDATE zuse_rust_github_users SET state='disconnected',sealed_credentials=NULL,epoch=epoch+1,version=version+1,updated_at=$2 WHERE actor_id=$1 AND epoch=$3 AND version=$4 AND state='revoking'").bind(&access.actor.account_id).bind(now()).bind(epoch).bind(version).execute(&mut *tx).await?;
        if removed.rows_affected() == 1 {
            sqlx::query("UPDATE zuse_rust_github_operations SET phase='cancelled',sealed_pending=NULL WHERE actor_id=$1 AND phase<>'complete'").bind(&access.actor.account_id).execute(&mut *tx).await?;
        }
        tx.commit().await?;
        Ok(())
    }
    pub async fn webhook(
        &self,
        signature: &str,
        delivery: &str,
        event: &str,
        body: &[u8],
    ) -> Result<(), ApiError> {
        verify_signature(self.config.webhook_secret.as_bytes(), signature, body)?;
        if delivery.is_empty()
            || delivery.len() > 200
            || !delivery
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        {
            return Err(ApiError::bad_request("invalid_github_delivery"));
        }
        let value: Value = serde_json::from_slice(body)
            .map_err(|_| ApiError::bad_request("invalid_github_event"))?;
        validate_webhook_installation(&self.config.app_id, event, &value)?;
        if matches!(
            event,
            "organization" | "membership" | "member" | "installation" | "installation_repositories"
        ) {
            self.membership_epoch
                .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        }
        let digest = URL_SAFE_NO_PAD.encode(Sha256::digest(body));
        sqlx::query("INSERT INTO zuse_rust_github_deliveries(delivery_id,body_hash,received_at) VALUES($1,$2,$3) ON CONFLICT DO NOTHING").bind(delivery).bind(&digest).bind(now()).execute(&self.pool).await?;
        let row = sqlx::query(
            "SELECT body_hash,completed FROM zuse_rust_github_deliveries WHERE delivery_id=$1",
        )
        .bind(delivery)
        .fetch_one(&self.pool)
        .await?;
        if row.try_get::<String, _>("body_hash")? != digest {
            return Err(ApiError::conflict("github_delivery_collision"));
        }
        if row.try_get::<bool, _>("completed")? {
            return Ok(());
        }
        if matches!(event, "installation" | "installation_repositories") {
            let id = value["installation"]["id"]
                .as_i64()
                .filter(|n| *n > 0)
                .ok_or_else(|| ApiError::bad_request("invalid_github_event"))?;
            let owners:Vec<String>=sqlx::query_scalar("SELECT owner_id FROM zuse_rust_github_installations WHERE installation_id=$1 AND state<>'removed'").bind(id).fetch_all(&self.pool).await?;
            for owner in owners {
                if self.enabled(&owner) {
                    self.observe(&owner, id).await?;
                }
            }
        }
        // Membership is revalidated using the user's own token on every authorization.
        // Webhooks deliberately never create memberships or installation links.
        let mut tx = self.pool.begin().await?;
        self.commit_membership_jobs(&mut tx, event, &value).await?;
        sqlx::query("UPDATE zuse_rust_github_deliveries SET completed=true WHERE delivery_id=$1 AND body_hash=$2").bind(delivery).bind(digest).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(())
    }
}
// Preserve the original App boundary even when a correctly signed request is
// routed to the wrong environment. Validate before any epoch, receipt or job.
fn validate_webhook_installation(app_id: &str, event: &str, value: &Value) -> Result<(), ApiError> {
    if !matches!(
        event,
        "organization" | "membership" | "member" | "installation" | "installation_repositories"
    ) {
        return Ok(());
    }
    let required = matches!(event, "installation" | "installation_repositories");
    let installation = &value["installation"];
    if required && !installation["id"].as_i64().is_some_and(|id| id > 0) {
        return Err(ApiError::bad_request("invalid_github_installation"));
    }
    match installation.get("app_id") {
        None if !required => Ok(()),
        Some(id)
            if id
                .as_u64()
                .zip(app_id.parse::<u64>().ok())
                .is_some_and(|(actual, expected)| actual > 0 && actual == expected) =>
        {
            Ok(())
        }
        _ => Err(ApiError::bad_request("invalid_github_installation")),
    }
}

fn parse_credentials(value: &Value, now: i64) -> Result<Credentials, ApiError> {
    let access = value["access_token"]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(reconnect)?;
    fn expiry(value: &Value, key: &str, now: i64) -> Result<Option<i64>, ApiError> {
        match value.get(key) {
            None => Ok(None),
            Some(v) => {
                let seconds = v
                    .as_i64()
                    .filter(|n| *n > 0 && *n <= 31_536_000)
                    .ok_or_else(reconnect)?;
                Ok(Some(now.checked_add(seconds * 1000).ok_or_else(reconnect)?))
            }
        }
    }
    let refresh = value["refresh_token"]
        .as_str()
        .filter(|s| !s.is_empty())
        .map(str::to_owned);
    let refresh_expiry = expiry(value, "refresh_token_expires_in", now)?;
    if refresh.is_some() && refresh_expiry.is_none() {
        return Err(reconnect());
    }
    Ok(Credentials {
        access_token: access.to_owned(),
        expires_at: expiry(value, "expires_in", now)?,
        refresh_token: refresh,
        refresh_expires_at: refresh_expiry,
    })
}
fn public_user(profile: &Value) -> Result<GithubUser, ApiError> {
    let id = profile["id"]
        .as_i64()
        .filter(|n| *n > 0)
        .ok_or_else(unavailable)?;
    let login = profile["login"]
        .as_str()
        .filter(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'))
        .ok_or_else(unavailable)?;
    let name = profile["name"]
        .as_str()
        .filter(|s| s.len() <= 256 && !s.chars().any(|c| c.is_control() || c == '<' || c == '>'))
        .unwrap_or(login);
    Ok(GithubUser {
        login: login.to_owned(),
        name: name.to_owned(),
        email: format!("{id}+{login}@users.noreply.github.com"),
    })
}
fn installation_summary(id: i64, metadata: &Value) -> Result<GithubInstallation, ApiError> {
    Ok(GithubInstallation {
        installation_id: id,
        account_login: metadata["account"]["login"]
            .as_str()
            .ok_or_else(unavailable)?
            .into(),
        account_type: metadata["account"]["type"]
            .as_str()
            .ok_or_else(unavailable)?
            .into(),
        avatar_url: metadata["account"]["avatar_url"]
            .as_str()
            .map(str::to_owned),
        repository_selection: metadata["repository_selection"]
            .as_str()
            .unwrap_or("selected")
            .into(),
        suspended: !metadata["suspended_at"].is_null(),
    })
}
fn repository_allowed(installation: &Value, allowed: Option<&Value>, repo: &Value) -> bool {
    let Some(full) = repo["full_name"].as_str() else {
        return false;
    };
    let Some((owner, name)) = full.split_once('/') else {
        return false;
    };
    valid_component(owner)
        && valid_component(name)
        && installation["account"]["login"]
            .as_str()
            .is_some_and(|s| s.eq_ignore_ascii_case(owner))
        && repo["permissions"]["push"] == true
        && allowed.is_none_or(|a| {
            a.as_array().is_some_and(|names| {
                names
                    .iter()
                    .any(|n| n.as_str().is_some_and(|s| s.eq_ignore_ascii_case(full)))
            })
        })
}
fn repository_summary(repo: &Value) -> Result<GithubRepository, ApiError> {
    let full = repo["full_name"].as_str().ok_or_else(unavailable)?;
    Ok(GithubRepository {
        name_with_owner: full.into(),
        description: repo["description"].as_str().map(str::to_owned),
        ssh_url: format!("git@github.com:{full}.git"),
        https_url: format!("https://github.com/{full}.git"),
        is_private: repo["private"].as_bool().unwrap_or(true),
        default_branch: repo["default_branch"].as_str().unwrap_or("main").into(),
        updated_at: repo["updated_at"].as_str().unwrap_or("").into(),
        owner_avatar_url: repo["owner"]["avatar_url"].as_str().map(str::to_owned),
    })
}
fn verify_signature(secret: &[u8], signature: &str, body: &[u8]) -> Result<(), ApiError> {
    if secret.is_empty() || body.len() > GITHUB_WEBHOOK_BODY_LIMIT {
        return Err(ApiError::unauthorized("invalid_github_signature"));
    }
    let hex = signature
        .strip_prefix("sha256=")
        .filter(|s| s.len() == 64)
        .ok_or_else(|| ApiError::unauthorized("invalid_github_signature"))?;
    let mut bytes = [0u8; 32];
    for (i, chunk) in hex.as_bytes().chunks_exact(2).enumerate() {
        let text = std::str::from_utf8(chunk)
            .map_err(|_| ApiError::unauthorized("invalid_github_signature"))?;
        bytes[i] = u8::from_str_radix(text, 16)
            .map_err(|_| ApiError::unauthorized("invalid_github_signature"))?;
    }
    let mut mac = Hmac::<Sha256>::new_from_slice(secret).map_err(|_| unavailable())?;
    mac.update(body);
    mac.verify_slice(&bytes)
        .map_err(|_| ApiError::unauthorized("invalid_github_signature"))
}

fn signing_key(pem: &[u8]) -> anyhow::Result<jsonwebtoken::EncodingKey> {
    use rsa::{
        pkcs1::{DecodeRsaPrivateKey, EncodeRsaPrivateKey},
        pkcs8::DecodePrivateKey,
    };
    let text =
        std::str::from_utf8(pem).map_err(|_| anyhow::anyhow!("Invalid GitHub RSA signing key"))?;
    let key = rsa::RsaPrivateKey::from_pkcs1_pem(text)
        .or_else(|_| rsa::RsaPrivateKey::from_pkcs8_pem(text))
        .map_err(|_| anyhow::anyhow!("Invalid GitHub RSA signing key"))?;
    let der = key
        .to_pkcs1_der()
        .map_err(|_| anyhow::anyhow!("Invalid GitHub RSA signing key"))?;
    Ok(jsonwebtoken::EncodingKey::from_rsa_der(der.as_bytes()))
}

// GitHub may issue a fresh scoped token whose lifetime exceeds the remaining
// parent lifetime. Never misrepresent the local authorization lease as upstream
// bearer expiry: GitHub alone controls the latter. The guest revalidates authority
// on credential-helper use (at most a ten-second coalescing window).
struct ScopedTokenExpiry {
    authority_expires_at: i64,
    provider_expires_at: i64,
}
fn scoped_token_expiry(
    value: &Value,
    parent: Option<i64>,
    at: i64,
) -> Result<ScopedTokenExpiry, ApiError> {
    let provider_expires_at = value["expires_at"]
        .as_str()
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|d| d.timestamp_millis())
        .ok_or_else(|| ApiError::unavailable("github_scoped_token_expiry_invalid"))?;
    if provider_expires_at <= at {
        return Err(ApiError::unavailable("github_scoped_token_expired"));
    }
    if parent.is_some_and(|expiry| expiry <= at) {
        return Err(reconnect());
    }
    Ok(ScopedTokenExpiry {
        authority_expires_at: provider_expires_at
            .min(parent.unwrap_or(i64::MAX))
            .min(at.saturating_add(3_600_000)),
        provider_expires_at,
    })
}
