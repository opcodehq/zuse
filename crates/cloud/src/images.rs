//! Owned account image builds. Existing ready images are immutable fallback records.
use crate::{access::Access, error::ApiError, jobs, store::Store};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Postgres, Transaction};
use std::{collections::HashSet, path::PathBuf, sync::Arc};
use zuse_cloud_runtime::image::{ImageManifest, ImagePlan, ImageRepository};
use zuse_protocol::cloud_projects::{GithubCatalog, ImageBuildMode, ImageBuildRequest};
pub(crate) mod deletion;
pub mod github;
pub mod worker;
pub const MIGRATION: &str = include_str!("../migrations/0007_images.sql");
#[derive(Clone, Serialize, Deserialize)]
pub struct ImageConfig {
    pub base_snapshot: String,
    pub base_version: u64,
    pub template_version: String,
    pub max_cache_bytes: u64,
}
impl ImageConfig {
    pub fn from_env() -> anyhow::Result<Option<Self>> {
        if std::env::var("ZUSE_CLOUD_IMAGE_WRITE_ACCOUNTS")
            .unwrap_or_default()
            .trim()
            .is_empty()
        {
            return Ok(None);
        }
        let config = Self {
            base_snapshot: std::env::var("ZUSE_CLOUD_RUST_IMAGE_BASE_SNAPSHOT")?,
            base_version: std::env::var("ZUSE_CLOUD_RUST_IMAGE_BASE_VERSION")?.parse()?,
            template_version: std::env::var("ZUSE_CLOUD_RUST_IMAGE_TEMPLATE_VERSION")?,
            max_cache_bytes: 4 * 1024 * 1024 * 1024,
        };
        anyhow::ensure!(
            !config.base_snapshot.is_empty()
                && config.base_version > 0
                && config.base_version <= 9_007_199_254_740_991
                && !config.template_version.is_empty(),
            "Invalid Rust base image configuration"
        );
        Ok(Some(config))
    }
}
#[derive(Clone)]
pub struct ImageControl {
    pub(crate) pool: PgPool,
    pub(crate) accounts: Arc<HashSet<String>>,
    pub(crate) config: ImageConfig,
}
#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct RepositoryPin {
    pub row: Value,
    pub installation: Option<i64>,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Operation {
    pub(crate) version: u32,
    pub(crate) build: String,
    pub(crate) account: String,
    pub(crate) phase: String,
    pub(crate) config: ImageConfig,
    pub(crate) plan: ImagePlan,
    pub(crate) repositories: Vec<RepositoryPin>,
    pub(crate) approved_until: i64,
    pub(crate) machine_name: String,
    pub(crate) snapshot_name: String,
    pub(crate) verify_name: String,
    pub(crate) machine: Option<String>,
    pub(crate) verifier: Option<String>,
    pub(crate) snapshot: Option<String>,
    pub(crate) snapshot_version: Option<u64>,
    pub(crate) manifest: Option<ImageManifest>,
    pub(crate) verified: bool,
    pub(crate) failure: Option<String>,
    pub(crate) created: i64,
    #[serde(default)]
    pub(crate) resources: Option<(u32, u64)>,
    #[serde(default)]
    pub(crate) verifier_started_at: Option<i64>,
}
fn unavailable() -> ApiError {
    ApiError::unavailable("cloud_image_unavailable")
}
pub(crate) fn text<'a>(row: &'a Value, key: &str) -> Result<&'a str, ApiError> {
    row[key].as_str().ok_or_else(unavailable)
}
fn now() -> i64 {
    chrono::Utc::now().timestamp_millis()
}
impl ImageControl {
    pub fn from_pool(pool: PgPool, accounts: Vec<String>, config: ImageConfig) -> Self {
        Self {
            pool,
            accounts: Arc::new(accounts.into_iter().collect()),
            config,
        }
    }
    pub async fn connect(
        url: &str,
        accounts: Vec<String>,
        config: ImageConfig,
    ) -> anyhow::Result<Self> {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(6)
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
            .map_err(|_| anyhow::anyhow!("Cannot connect cloud image writer"))?;
        Ok(Self::from_pool(pool, accounts, config))
    }
    pub(crate) async fn owned(&self, account: &str) -> Result<Transaction<'_, Postgres>, ApiError> {
        if !self.accounts.contains(account) {
            return Err(ApiError::forbidden("cloud_image_writes_disabled"));
        }
        let mut tx = self.pool.begin().await?;
        for kind in ["project-catalog", "image-catalog"] {
            let backend:Option<String>=sqlx::query_scalar("SELECT backend FROM zuse_rust_control_owners WHERE resource_kind=$1 AND resource_id=$2 FOR SHARE").bind(kind).bind(account).fetch_optional(&mut *tx).await?;
            if backend.as_deref() != Some("rust") {
                return Err(ApiError::conflict("cloud_image_owner_not_rust"));
            }
        }
        Ok(tx)
    }
    async fn admit(
        &self,
        access: &Access,
        request: ImageBuildRequest,
        github: Option<GithubCatalog>,
        at: i64,
    ) -> Result<(), ApiError> {
        let account = &access.owner_id;
        if request.provider_id.as_deref().is_some_and(|p| p != "boxd") {
            return Err(ApiError::conflict("cloud_provider_unavailable"));
        }
        if request.idempotency_key.is_empty() || request.idempotency_key.len() > 1024 {
            return Err(ApiError::bad_request("invalid_image_build_request"));
        }
        let mut tx = self.owned(account).await?;
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
            .bind(format!("project-catalog:{account}"))
            .execute(&mut *tx)
            .await?;
        let mode = match request.mode {
            ImageBuildMode::Update => "update",
            ImageBuildMode::Rebuild => "rebuild",
        };
        let key = format!("account-image:{mode}:{}", request.idempotency_key);
        let prior:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM api_cloud_project_builds WHERE account_id=$1 AND provider='boxd' AND idempotency_key=$2)").bind(account).bind(&key).fetch_one(&mut *tx).await?;
        if prior {
            tx.commit().await?;
            return Ok(());
        }
        let entitled:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM api_entitlements WHERE account_id=$1 AND kind='cloud-workspace' AND (status IN ('active','grace') OR (status='ended' AND paid_through>$2)) AND (paid_through IS NULL OR paid_through>$2))").bind(account).bind(at).fetch_one(&mut *tx).await?;
        if !entitled {
            return Err(ApiError::forbidden("cloud_entitlement_required"));
        }
        require_billing_owner(&mut tx, account).await?;
        crate::billing::require_capacity(&mut tx, account, at).await?;
        let pending:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM api_cloud_project_builds WHERE account_id=$1 AND provider='boxd' AND state IN ('queued','building','sanitizing'))").bind(account).fetch_one(&mut *tx).await?;
        if pending {
            tx.commit().await?;
            return Ok(());
        }
        let rows:Vec<Value>=sqlx::query_scalar("SELECT to_jsonb(p) FROM api_cloud_projects p WHERE account_id=$1 AND included=true ORDER BY project_id FOR SHARE").bind(account).fetch_all(&mut *tx).await?;
        if rows.is_empty() {
            return Err(ApiError::conflict("cloud_image_has_no_repositories"));
        }
        let build = format!("image_{}", uuid::Uuid::new_v4().simple());
        let suffix = build.trim_start_matches("image_");
        let mut pins = Vec::new();
        let mut repos = Vec::new();
        for row in &rows {
            if row["secret_bindings"]
                .as_array()
                .is_some_and(|b| !b.is_empty())
            {
                return Err(ApiError::conflict("cloud_project_secret_vault_required"));
            }
            let identity = text(row, "repository_identity")?;
            let private = row["visibility"] == "private";
            let installation = if private {
                let catalog = github
                    .as_ref()
                    .ok_or_else(|| ApiError::conflict("cloud_github_repository_access_required"))?;
                let name = identity
                    .strip_prefix("github.com/")
                    .ok_or_else(|| ApiError::conflict("cloud_github_repository_access_required"))?;
                if !catalog
                    .repositories
                    .iter()
                    .any(|r| r.name_with_owner.eq_ignore_ascii_case(name))
                {
                    return Err(ApiError::forbidden(
                        "cloud_github_repository_access_required",
                    ));
                }
                let owner = name.split('/').next().unwrap();
                let install = catalog
                    .installations
                    .iter()
                    .find(|i| !i.suspended && i.account_login.eq_ignore_ascii_case(owner))
                    .ok_or_else(|| {
                        ApiError::forbidden("cloud_github_repository_access_required")
                    })?;
                let allowed:Option<Value>=sqlx::query_scalar("SELECT to_jsonb(i) FROM api_cloud_github_installations i WHERE account_id=$1 AND installation_id=$2 AND suspended=false FOR SHARE").bind(account).bind(install.installation_id).fetch_optional(&mut *tx).await?;
                let allowed = allowed.ok_or_else(|| {
                    ApiError::forbidden("cloud_github_repository_access_required")
                })?;
                if allowed["allowed_repositories"]
                    .as_array()
                    .is_some_and(|items| {
                        !items
                            .iter()
                            .any(|n| n.as_str().is_some_and(|n| n.eq_ignore_ascii_case(name)))
                    })
                {
                    return Err(ApiError::forbidden(
                        "cloud_github_repository_access_required",
                    ));
                }
                Some(install.installation_id)
            } else {
                None
            };
            repos.push(ImageRepository {
                project_id: text(row, "project_id")?.into(),
                repository_identity: identity.into(),
                repository_url: text(row, "repository_url")?.into(),
                default_branch: text(row, "default_branch")?.into(),
                token_file: installation
                    .map(|id| PathBuf::from(format!("/run/zuse-secrets/github-installation-{id}"))),
            });
            pins.push(RepositoryPin {
                row: json!({"project_id":row["project_id"],"repository_identity":row["repository_identity"],"configuration_digest":row["configuration_digest"]}),
                installation,
            });
        }
        let digest = configuration_digest(mode, &self.config.template_version, &rows)?;
        let plan = ImagePlan {
            build_id: build.clone(),
            template_version: self.config.template_version.clone(),
            configuration_digest: digest.clone(),
            home: "/home/zuse".into(),
            cache_root: "/home/repos".into(),
            status_directory: format!("/var/lib/zuse/project-build/{build}").into(),
            secrets_root: "/run/zuse-secrets".into(),
            max_cache_bytes: self.config.max_cache_bytes,
            repositories: repos,
        };
        let operation = Operation {
            version: 1,
            build: build.clone(),
            account: account.clone(),
            phase: "allocate".into(),
            config: self.config.clone(),
            plan,
            repositories: pins,
            approved_until: at + 15 * 60_000,
            machine_name: format!("zuse-build-{suffix}"),
            snapshot_name: format!("zuse-image-{suffix}"),
            verify_name: format!("zuse-check-{suffix}"),
            machine: None,
            verifier: None,
            snapshot: None,
            snapshot_version: None,
            manifest: None,
            verified: false,
            failure: None,
            created: at,
            resources: None,
            verifier_started_at: None,
        };
        let settings = json!({"mode":mode,"providerAuthDeliveryVersion":1,"repositories":public_repositories(&rows),"rustImageBuildVersion":1,"rustProjectConfigurationDigests":rows.iter().map(|r|(r["project_id"].as_str().unwrap_or("").to_owned(),r["configuration_digest"].clone())).collect::<serde_json::Map<_,_>>()});
        sqlx::query("INSERT INTO api_cloud_project_builds(build_id,project_id,account_id,provider,template_version,configuration_digest,settings,state,idempotency_key,next_action_at,revision,created_at,updated_at) VALUES($1,$2,$3,'boxd',$4,$5,$6,'queued',$7,$8,0,$8,$8)")
            .bind(&build).bind(text(&rows[0],"project_id")?).bind(account).bind(&self.config.template_version).bind(digest).bind(settings).bind(key).bind(at).execute(&mut *tx).await?;
        sqlx::query("INSERT INTO zuse_rust_image_operations VALUES($1,$2,$3,$4,$4)")
            .bind(&build)
            .bind(account)
            .bind(serde_json::to_value(&operation).map_err(|_| unavailable())?)
            .bind(at)
            .execute(&mut *tx)
            .await?;
        sqlx::query("INSERT INTO zuse_rust_control_owners VALUES('image-build',$1,'rust',1,$2)")
            .bind(&build)
            .bind(at)
            .execute(&mut *tx)
            .await?;
        jobs::enqueue(
            &mut tx,
            &format!("image:{build}"),
            "image-build",
            &build,
            1,
            &json!({"accountId":account}),
            at,
        )
        .await?;
        sqlx::query("UPDATE api_cloud_projects SET state='preparing',updated_at=$2 WHERE account_id=$1 AND included=true AND state<>'ready'").bind(account).bind(at).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(())
    }
}
pub(crate) fn public_repositories(rows: &[Value]) -> Vec<Value> {
    rows.iter().map(|r|json!({"projectId":r["project_id"],"repositoryIdentity":r["repository_identity"],"displayName":r["display_name"],"defaultBranch":r["default_branch"]})).collect()
}
impl Store {
    pub fn with_image_control(mut self, control: ImageControl) -> Self {
        self.images = Some(Arc::new(control));
        self
    }
    pub fn image_writes_enabled(&self) -> bool {
        self.images.is_some()
    }
    pub async fn image_build(
        &self,
        access: &Access,
        request: ImageBuildRequest,
        github: Option<GithubCatalog>,
        at: i64,
    ) -> Result<Value, ApiError> {
        self.images
            .as_ref()
            .ok_or_else(|| ApiError::unavailable("cloud_image_writes_disabled"))?
            .admit(access, request, github, at)
            .await?;
        self.image_status(&access.owner_id, true).await
    }
    pub async fn image_status(&self, account: &str, logs: bool) -> Result<Value, ApiError> {
        let rows:Vec<Value>=sqlx::query_scalar("SELECT to_jsonb(b) FROM ((SELECT * FROM api_cloud_project_builds WHERE account_id=$1 AND provider='boxd' ORDER BY created_at DESC,build_id DESC LIMIT 12) UNION (SELECT * FROM api_cloud_project_builds WHERE account_id=$1 AND provider='boxd' AND state='ready' AND NOT EXISTS(SELECT 1 FROM api_cloud_snapshots s WHERE s.account_id=api_cloud_project_builds.account_id AND s.provider IN ('box','boxd') AND s.snapshot_id=api_cloud_project_builds.snapshot_id AND s.state IN ('deleting','deleted')) ORDER BY updated_at DESC,build_id DESC LIMIT 1) UNION (SELECT * FROM api_cloud_project_builds WHERE account_id=$1 AND provider='boxd' AND state IN ('queued','building','sanitizing') ORDER BY created_at DESC LIMIT 1)) b ORDER BY created_at DESC,build_id DESC").bind(account).fetch_all(&self.pool).await?;
        let projects:Vec<Value>=sqlx::query_scalar("SELECT to_jsonb(p) FROM api_cloud_projects p WHERE account_id=$1 AND included=true ORDER BY project_id").bind(account).fetch_all(&self.pool).await?;
        let ids = rows
            .iter()
            .filter_map(|r| r["snapshot_id"].as_str().map(str::to_owned))
            .collect::<Vec<_>>();
        let snapshots:Vec<Value>=sqlx::query_scalar("SELECT COALESCE(to_jsonb(s)->'record',jsonb_build_object('snapshotId',s.snapshot_id,'state',s.state,'accountId',s.account_id)) FROM api_cloud_snapshots s WHERE account_id=$1 AND provider IN ('box','boxd') AND (state IN ('retained','deleting') OR snapshot_id=ANY($2)) ORDER BY (to_jsonb(s)->>'created_at')::bigint DESC").bind(account).bind(ids).fetch_all(&self.pool).await?;
        let active = rows
            .iter()
            .filter(|r| {
                r["state"] == "ready"
                    && !snapshots.iter().any(|s| {
                        s["snapshotId"] == r["snapshot_id"]
                            && matches!(s["state"].as_str(), Some("deleting" | "deleted"))
                    })
            })
            .max_by_key(|r| r["updated_at"].as_i64().unwrap_or(0));
        let building = rows.iter().find(|r| {
            matches!(
                r["state"].as_str(),
                Some("queued" | "building" | "sanitizing")
            )
        });
        let latest = rows.first();
        let failed = latest.is_some_and(|r| {
            r["state"] == "failed"
                && active.is_none_or(|a| r["created_at"].as_i64() >= a["updated_at"].as_i64())
        });
        let current = self
            .images
            .as_ref()
            .map(|c| c.config.template_version.as_str())
            .or_else(|| self.project_templates.get("boxd").map(String::as_str));
        let outdated = active.is_some_and(|a| {
            a["settings"]["runtimeRpcProtocol"] != "zuse-rpc-v1"
                || current.is_some_and(|v| a["template_version"] != v)
                || projects.iter().any(|p| {
                    if a["settings"]["rustProjectConfigurationDigests"].is_object()
                        && a["settings"]["rustProjectConfigurationDigests"]
                            [p["project_id"].as_str().unwrap_or("")]
                            != p["configuration_digest"]
                    {
                        return true;
                    }
                    !a["settings"]["repositories"].as_array().is_some_and(|rs| {
                        rs.iter().any(|r| {
                            r["projectId"] == p["project_id"]
                                && r["defaultBranch"] == p["default_branch"]
                        })
                    })
                })
                || a["settings"]["repositories"]
                    .as_array()
                    .is_some_and(|rs| rs.len() != projects.len())
        });
        let state = if building.is_some() {
            "building"
        } else if failed {
            "failed"
        } else if active.is_none() {
            "not-built"
        } else if outdated {
            "outdated"
        } else {
            "ready"
        };
        let status = building.or(active).or(latest);
        let repositories = status
            .and_then(|r| r["settings"]["repositories"].as_array().cloned())
            .unwrap_or_else(|| public_repositories(&projects));
        let mut result = json!({"state":state,"source":"managed","providerId":"boxd","repositories":repositories,"providers":[],"builds":rows.iter().take(12).map(|r|{
            let mut value=json!({"source":"managed","buildId":r["build_id"],"state":r["state"],"mode":r["settings"]["mode"].as_str().unwrap_or("update"),"active":active.is_some_and(|a|a["build_id"]==r["build_id"]),"runtimeVersion":r["template_version"],"configurationDigest":r["configuration_digest"],"repositories":r["settings"]["repositories"].as_array().cloned().unwrap_or_default(),"providers":[],"createdAt":r["created_at"],"updatedAt":r["updated_at"]});
            for (key,to)in[("last_error_code","errorCode"),("log_text","logText")]{if !r[key].is_null()&&(key!="log_text"||logs){value[to]=r[key].clone();}}
            if let Some(phase)=r["settings"]["rustImagePhase"].as_str(){value["progressPhase"]=json!(phase);}value
        }).collect::<Vec<_>>(),"updatedAt":status.and_then(|r|r["updated_at"].as_i64()).unwrap_or(0)});
        if let Some(active) = active {
            result["generation"] = active["build_id"].clone();
            result["builtAt"] = active["updated_at"].clone();
            result["runtimeVersion"] = active["template_version"].clone();
            if active["settings"]["providerAuthDeliveryVersion"] == 1 {
                result["providerAuthDeliveryVersion"] = json!(1);
            }
        }
        if let Some(building) = building {
            result["progressPhase"] = building["settings"]["rustImagePhase"]
                .as_str()
                .unwrap_or("queued")
                .into();
        }
        if failed {
            result["errorCode"] = latest.unwrap()["last_error_code"].clone();
        }
        if let Some(storage) = snapshots
            .iter()
            .find(|s| s["state"] == "retained")
            .or_else(|| snapshots.iter().find(|s| s["state"] == "deleting"))
        {
            let enabled = crate::billing::SnapshotSettlementConfig::from_env()
                .map_err(|_| unavailable())?
                .is_some_and(|c| now() >= c.cutover_at_ms);
            result["storage"] = json!({"snapshotId":storage["snapshotId"],"state":storage["state"],"monthlyCostMicros":1_700_000,"billingEnabled":enabled});
            if storage["lastError"].is_string() {
                result["storage"]["lastError"] = storage["lastError"].clone();
            }
        }
        Ok(result)
    }
}

fn configuration_digest(mode: &str, template: &str, rows: &[Value]) -> Result<String, ApiError> {
    // Field order is the pinned TypeScript JSON.stringify contract.
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct Repository<'a> {
        project_id: &'a str,
        configuration_digest: &'a str,
    }
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct Configuration<'a> {
        mode: &'a str,
        template_version: &'a str,
        codex_auth_delivery_version: u8,
        provider_auth_delivery_version: u8,
        repositories: Vec<Repository<'a>>,
    }
    let mut repositories = rows
        .iter()
        .map(|r| {
            Ok(Repository {
                project_id: text(r, "project_id")?,
                configuration_digest: text(r, "configuration_digest")?,
            })
        })
        .collect::<Result<Vec<_>, ApiError>>()?;
    repositories.sort_by(|a, b| a.project_id.cmp(b.project_id));
    Ok(format!(
        "{:x}",
        Sha256::digest(
            serde_json::to_vec(&Configuration {
                mode,
                template_version: template,
                codex_auth_delivery_version: 0,
                provider_auth_delivery_version: 1,
                repositories
            })
            .map_err(|_| unavailable())?
        )
    ))
}
#[cfg(test)]
mod tests;

async fn require_billing_owner(
    tx: &mut Transaction<'_, Postgres>,
    account: &str,
) -> Result<(), ApiError> {
    let owner:Option<String>=sqlx::query_scalar("SELECT backend FROM zuse_rust_control_owners WHERE resource_kind='billing' AND resource_id=$1 FOR SHARE").bind(account).fetch_optional(&mut **tx).await?;
    if owner.as_deref() != Some("rust") {
        return Err(ApiError::conflict("cloud_image_billing_owner_not_rust"));
    }
    Ok(())
}
