//! Original repository catalog; removal changes selection, never workspace storage.
use crate::{error::ApiError, store::Store};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Postgres, Transaction};
use std::{
    collections::{BTreeMap, HashSet},
    sync::Arc,
};
use zuse_protocol::cloud_projects::*;
#[derive(Clone)]
pub struct ProjectControl {
    pool: PgPool,
    accounts: Arc<HashSet<String>>,
}
fn invalid() -> ApiError {
    ApiError::bad_request("invalid_repository")
}
fn catalog_invalid() -> ApiError {
    ApiError::unavailable("cloud_project_catalog_invalid")
}
pub fn template_versions_from_env() -> anyhow::Result<BTreeMap<String, String>> {
    let raw =
        std::env::var("ZUSE_CLOUD_PROVIDER_TEMPLATE_VERSIONS").unwrap_or_else(|_| "{}".into());
    let versions: BTreeMap<String, String> = serde_json::from_str(&raw)?;
    anyhow::ensure!(
        versions.iter().all(|(provider, version)| matches!(
            provider.as_str(),
            "box" | "boxd" | "e2b"
        ) && !version.trim().is_empty()
            && version.len() <= 1024),
        "Invalid cloud provider template versions"
    );
    Ok(versions)
}
impl ProjectControl {
    pub fn from_pool(pool: PgPool, accounts: Vec<String>) -> Self {
        Self {
            pool,
            accounts: Arc::new(accounts.into_iter().collect()),
        }
    }
    pub async fn connect(url: &str, accounts: Vec<String>) -> anyhow::Result<Self> {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(4)
            .after_connect(|conn, _| {
                Box::pin(async move {
                    sqlx::query("SET statement_timeout='10s'")
                        .execute(&mut *conn)
                        .await?;
                    sqlx::query("SET lock_timeout='5s'")
                        .execute(&mut *conn)
                        .await?;
                    Ok(())
                })
            })
            .connect(url)
            .await
            .map_err(|_| anyhow::anyhow!("Cannot connect cloud project writer"))?;
        Ok(Self::from_pool(pool, accounts))
    }
    async fn owned(&self, owner: &str) -> Result<Transaction<'_, Postgres>, ApiError> {
        if !self.accounts.contains(owner) {
            return Err(ApiError::forbidden("cloud_project_writes_disabled"));
        }
        let mut tx = self.pool.begin().await?;
        let backend:Option<String>=sqlx::query_scalar("SELECT backend FROM zuse_rust_control_owners WHERE resource_kind='project-catalog' AND resource_id=$1 FOR SHARE").bind(owner).fetch_optional(&mut *tx).await?;
        if backend.as_deref() != Some("rust") {
            return Err(ApiError::conflict("cloud_project_owner_not_rust"));
        }
        // Serialize idempotency across repositories and selection changes for this account.
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
            .bind(format!("project-catalog:{owner}"))
            .execute(&mut *tx)
            .await?;
        Ok(tx)
    }
    async fn connect_project(
        &self,
        owner: &str,
        request: ProjectConnectRequest,
        now: i64,
    ) -> Result<Value, ApiError> {
        let normalized = normalize(&request.repository_url)?;
        validate(&request)?;
        if !request.secret_bindings.is_empty() {
            return Err(ApiError::conflict("cloud_project_secret_vault_required"));
        }
        let mut tx = self.owned(owner).await?;
        let entitled:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM api_entitlements WHERE account_id=$1 AND kind='cloud-workspace' AND (status IN ('active','grace') OR (status='ended' AND paid_through>$2)) AND (paid_through IS NULL OR paid_through>$2))").bind(owner).bind(now).fetch_one(&mut *tx).await?;
        if !entitled {
            return Err(ApiError::forbidden("cloud_entitlement_required"));
        }
        // Keep the original repository lock as well, for compatibility during fenced handoff.
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
            .bind(format!("{owner}:{}", normalized.identity))
            .execute(&mut *tx)
            .await?;
        let retry:Option<Value>=sqlx::query_scalar("SELECT to_jsonb(p) FROM api_cloud_projects p WHERE account_id=$1 AND idempotency_key=$2 AND included=true LIMIT 1 FOR UPDATE").bind(owner).bind(&request.idempotency_key).fetch_optional(&mut *tx).await?;
        if let Some(retry) = retry {
            tx.commit().await?;
            return Ok(retry);
        }
        // A removed row may own this key for a different repository. Leave it intact.
        let occupied:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM api_cloud_projects WHERE account_id=$1 AND idempotency_key=$2 AND repository_identity<>$3)").bind(owner).bind(&request.idempotency_key).bind(&normalized.identity).fetch_one(&mut *tx).await?;
        if occupied {
            return Err(ApiError::conflict("idempotency_key_reused"));
        }
        let id = format!("project_{}", uuid::Uuid::new_v4().simple());
        let digest = configuration_digest(&request.default_branch);
        let visibility = match request.visibility {
            ProjectVisibility::Public => "public",
            ProjectVisibility::Private => "private",
        };
        let row:Value=sqlx::query_scalar("INSERT INTO api_cloud_projects AS p(project_id,account_id,repository_identity,repository_url,display_name,default_branch,visibility,git_connection_kind,cloud_environment,secret_bindings,configuration_digest,state,included,last_error_code,idempotency_key,created_at,updated_at) VALUES($1,$2,$3,$4,$5,$6,$7,'github-app',$8,'[]',$9,'connected',true,NULL,$10,$11,$11) ON CONFLICT(account_id,repository_identity) DO UPDATE SET repository_url=EXCLUDED.repository_url,display_name=EXCLUDED.display_name,default_branch=EXCLUDED.default_branch,visibility=EXCLUDED.visibility,cloud_environment=EXCLUDED.cloud_environment,secret_bindings=EXCLUDED.secret_bindings,configuration_digest=EXCLUDED.configuration_digest,state='connected',included=true,last_error_code=NULL,idempotency_key=EXCLUDED.idempotency_key,updated_at=EXCLUDED.updated_at RETURNING to_jsonb(p)")
            .bind(id).bind(owner).bind(normalized.identity).bind(normalized.url).bind(request.display_name.unwrap_or(normalized.name)).bind(request.default_branch).bind(visibility).bind(json!(request.cloud_environment)).bind(digest).bind(request.idempotency_key).bind(now).fetch_one(&mut *tx).await?;
        tx.commit().await?;
        Ok(row)
    }
    async fn remove(&self, owner: &str, id: &str, now: i64) -> Result<Value, ApiError> {
        let mut tx = self.owned(owner).await?;
        let row:Option<Value>=sqlx::query_scalar("UPDATE api_cloud_projects AS p SET included=false,updated_at=$3 WHERE project_id=$1 AND account_id=$2 RETURNING to_jsonb(p)").bind(id).bind(owner).bind(now).fetch_optional(&mut *tx).await?;
        let row = row.ok_or_else(|| ApiError::not_found("cloud_project_not_found"))?;
        tx.commit().await?;
        Ok(row)
    }
}
struct Repository {
    identity: String,
    url: String,
    name: String,
}
fn normalize(raw: &str) -> Result<Repository, ApiError> {
    let trimmed = raw.trim();
    let raw = if trimmed.contains("://") {
        trimmed.to_owned()
    } else {
        format!("https://{trimmed}")
    };
    let url = reqwest::Url::parse(&raw).map_err(|_| invalid())?;
    if url.scheme() != "https" || !url.username().is_empty() || url.password().is_some() {
        return Err(invalid());
    }
    let host = url.host_str().ok_or_else(invalid)?;
    let path = url.path().trim_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    let parts = path.split('/').collect::<Vec<_>>();
    if parts.len() != 2
        || parts.iter().any(|part| {
            part.is_empty()
                || !part
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"_.-".contains(&c))
        })
    {
        return Err(invalid());
    }
    Ok(Repository {
        identity: format!(
            "{}/{}/{}",
            host.to_lowercase(),
            parts[0].to_lowercase(),
            parts[1].to_lowercase()
        ),
        url: format!("https://{host}/{}/{}.git", parts[0], parts[1]),
        name: parts[1].into(),
    })
}
fn validate(r: &ProjectConnectRequest) -> Result<(), ApiError> {
    if r.repository_url.len() > 8192
        || r.default_branch.is_empty()
        || r.default_branch.len() > 1024
        || !r
            .default_branch
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"._/-".contains(&c))
        || r.idempotency_key.is_empty()
        || r.idempotency_key.len() > 1024
        || r.display_name.as_ref().is_some_and(|s| s.len() > 1024)
    {
        return Err(invalid());
    }
    if r.cloud_environment.iter().any(|(key, value)| {
        !key.as_bytes()
            .first()
            .is_some_and(|b| b.is_ascii_uppercase() || *b == b'_')
            || !key
                .bytes()
                .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
            || [
                "TOKEN",
                "SECRET",
                "PASSWORD",
                "PRIVATE",
                "CREDENTIAL",
                "API_KEY",
            ]
            .iter()
            .any(|word| key.contains(word))
            || value.encode_utf16().count() > 8192
    }) {
        return Err(invalid());
    }
    Ok(())
}
fn configuration_digest(branch: &str) -> String {
    format!(
        "{:x}",
        Sha256::digest(
            serde_json::to_vec(
                &json!({"defaultBranch":branch,"preparationMode":"account-repository-cache-v1"})
            )
            .expect("JSON primitives")
        )
    )
}
impl Store {
    pub fn with_project_control(mut self, control: ProjectControl) -> Self {
        self.projects = Some(Arc::new(control));
        self
    }
    pub fn with_project_templates(mut self, versions: BTreeMap<String, String>) -> Self {
        self.project_templates = Arc::new(versions);
        self
    }
    pub fn project_writes_enabled(&self) -> bool {
        self.projects.is_some()
    }
    pub async fn project_connect(
        &self,
        owner: &str,
        request: ProjectConnectRequest,
        now: i64,
    ) -> Result<CloudProject, ApiError> {
        let row = self
            .projects
            .as_ref()
            .ok_or_else(|| ApiError::unavailable("cloud_project_writes_disabled"))?
            .connect_project(owner, request, now)
            .await?;
        public_project(&row, &[], &self.project_templates)
    }
    pub async fn project_remove(
        &self,
        owner: &str,
        id: &str,
        now: i64,
    ) -> Result<CloudProject, ApiError> {
        let row = self
            .projects
            .as_ref()
            .ok_or_else(|| ApiError::unavailable("cloud_project_writes_disabled"))?
            .remove(owner, id, now)
            .await?;
        public_project(&row, &[], &self.project_templates)
    }
    pub async fn project_list(&self, owner: &str) -> Result<ProjectList, ApiError> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
            .execute(&mut *tx)
            .await?;
        let rows:Vec<Value>=sqlx::query_scalar("SELECT to_jsonb(p) FROM api_cloud_projects p WHERE account_id=$1 AND included=true ORDER BY created_at,project_id").bind(owner).fetch_all(&mut *tx).await?;
        let builds:Vec<Value>=sqlx::query_scalar("SELECT to_jsonb(b) FROM api_cloud_project_builds b WHERE account_id=$1 ORDER BY created_at,build_id").bind(owner).fetch_all(&mut *tx).await?;
        let mut projects = Vec::new();
        for row in rows {
            let relevant = builds
                .iter()
                .filter(|b| {
                    b["project_id"] == row["project_id"]
                        || (b["settings"]["source"] == "custom-snapshot"
                            && b["settings"]["repositories"]
                                .as_array()
                                .is_some_and(|repos| {
                                    repos.iter().any(|r| {
                                        r["projectId"] == row["project_id"]
                                            && r["path"].as_str().is_some_and(safe_path)
                                    })
                                })
                            && b["settings"]["runtimeHome"].as_str().is_some_and(safe_path))
                })
                .cloned()
                .collect::<Vec<_>>();
            projects.push(public_project(&row, &relevant, &self.project_templates)?);
        }
        tx.commit().await?;
        Ok(ProjectList { projects })
    }
}
fn safe_path(path: &str) -> bool {
    path.starts_with('/') && !path.contains(['\0', '\r', '\n'])
}
fn text(row: &Value, key: &str) -> Result<String, ApiError> {
    row[key]
        .as_str()
        .map(str::to_owned)
        .ok_or_else(catalog_invalid)
}
fn time(row: &Value, key: &str) -> Result<i64, ApiError> {
    row[key].as_i64().ok_or_else(catalog_invalid)
}
fn public_project(
    row: &Value,
    builds: &[Value],
    versions: &BTreeMap<String, String>,
) -> Result<CloudProject, ApiError> {
    let mut active_builds = BTreeMap::new();
    let mut latest_builds = BTreeMap::new();
    let mut active_times: BTreeMap<String, i64> = BTreeMap::new();
    for build in builds {
        let provider = text(build, "provider")?;
        let created = time(build, "created_at")?;
        if latest_builds
            .get(&provider)
            .is_none_or(|b: &ProjectBuildStatus| b.created_at < created)
        {
            latest_builds.insert(
                provider.clone(),
                ProjectBuildStatus {
                    build_id: text(build, "build_id")?,
                    provider_id: provider.clone(),
                    state: text(build, "state")?,
                    error_code: build["last_error_code"].as_str().map(str::to_owned),
                    created_at: created,
                    updated_at: time(build, "updated_at")?,
                },
            );
        }
        let compatible = versions.get(&provider).is_some_and(|current| {
            if build["settings"]["source"] == "custom-snapshot" {
                build["settings"]["providerConnectionId"]
                    .as_str()
                    .is_some_and(|id| current.ends_with(&format!(":connection:{id}")))
            } else {
                build["template_version"] == current.as_str()
            }
        });
        let updated = time(build, "updated_at")?;
        if build["state"] == "ready"
            && compatible
            && active_times
                .get(&provider)
                .is_none_or(|prior| *prior <= updated)
        {
            active_times.insert(provider.clone(), updated);
            active_builds.insert(provider, text(build, "build_id")?);
        }
    }
    Ok(CloudProject {
        project_id: text(row, "project_id")?,
        repository_identity: text(row, "repository_identity")?,
        repository_url: text(row, "repository_url")?,
        display_name: text(row, "display_name")?,
        default_branch: text(row, "default_branch")?,
        visibility: serde_json::from_value(row["visibility"].clone())
            .map_err(|_| catalog_invalid())?,
        state: text(row, "state")?,
        active_builds,
        latest_builds,
        created_at: time(row, "created_at")?,
        updated_at: time(row, "updated_at")?,
    })
}
#[cfg(test)]
mod tests;
