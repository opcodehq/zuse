//! Computer setup reads do not allocate machines. Environment writes use optimistic revisions.
use crate::{error::ApiError, store::Store};
use serde_json::Value;
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use std::{collections::HashSet, sync::Arc};
use zuse_protocol::cloud_computers::*;
mod machine;
#[cfg(test)]
mod tests;

pub struct ComputerControl {
    pub(crate) pool: PgPool,
    pub(crate) cache_schema_ready: std::sync::OnceLock<bool>,
    accounts: HashSet<String>,
}
impl ComputerControl {
    pub async fn connect(url: &str, accounts: Vec<String>) -> anyhow::Result<Self> {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(4)
            .after_connect(|connection, _| {
                Box::pin(async move {
                    sqlx::query("SET statement_timeout='10s'")
                        .execute(&mut *connection)
                        .await?;
                    sqlx::query("SET lock_timeout='5s'")
                        .execute(connection)
                        .await?;
                    Ok(())
                })
            })
            .connect(url)
            .await
            .map_err(|_| anyhow::anyhow!("Cannot connect computer setup database"))?;
        Ok(Self::from_pool(pool, accounts))
    }
    pub fn from_pool(pool: PgPool, accounts: Vec<String>) -> Self {
        Self {
            pool,
            cache_schema_ready: std::sync::OnceLock::new(),
            accounts: accounts.into_iter().collect(),
        }
    }
    pub async fn save_environment(
        &self,
        owner: &str,
        project: &str,
        request: SaveEnvironment,
        now: i64,
    ) -> Result<EnvironmentVersion, ApiError> {
        if owner.starts_with("organization:") {
            return Err(ApiError::forbidden("environment_actor_required"));
        }
        self.save_environment_as(owner, owner, None, project, request, now)
            .await
    }
    pub async fn save_environment_scoped(
        &self,
        access: &crate::access::Access,
        project: &str,
        request: SaveEnvironment,
        now: i64,
    ) -> Result<EnvironmentVersion, ApiError> {
        let actor=access.membership.as_ref().map(|m|serde_json::json!({"subject":access.actor.account_id,"membershipId":m.id,"organizationId":m.organization_id,"role":m.role.slug}));
        self.save_environment_as(
            &access.owner_id,
            &access.actor.account_id,
            actor,
            project,
            request,
            now,
        )
        .await
    }
    async fn save_environment_as(
        &self,
        owner: &str,
        actor: &str,
        actor_metadata: Option<Value>,
        project: &str,
        request: SaveEnvironment,
        now: i64,
    ) -> Result<EnvironmentVersion, ApiError> {
        if !self.accounts.contains(owner) {
            return Err(ApiError::forbidden("cloud_environment_writes_disabled"));
        }
        request.recipe.validate().map_err(ApiError::bad_request)?;
        if request.expected_revision < 0 || request.expected_revision == i64::MAX {
            return Err(ApiError::bad_request("invalid_environment_revision"));
        }
        let mut tx = self.pool.begin().await?;
        let repository:Option<(String,String)>=sqlx::query_as("SELECT repository_identity,default_branch FROM api_cloud_projects WHERE account_id=$1 AND project_id=$2 AND included=true FOR SHARE").bind(owner).bind(project).fetch_optional(&mut *tx).await?;
        let Some((repository_identity, requested_ref)) = repository else {
            return Err(ApiError::not_found("cloud_project_not_found"));
        };
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
            .bind(format!("environment:{owner}:{project}"))
            .execute(&mut *tx)
            .await?;
        let current:Option<(i64,String)>=sqlx::query_as("SELECT revision,source FROM cloud_environment_versions WHERE owner_id=$1 AND project_id=$2 ORDER BY revision DESC LIMIT 1 FOR UPDATE").bind(owner).bind(project).fetch_optional(&mut *tx).await?;
        if current
            .as_ref()
            .is_some_and(|(_, source)| source == "repository")
        {
            return Err(ApiError::conflict("environment_managed_in_repository"));
        }
        if current.map(|(revision, _)| revision).unwrap_or(0) != request.expected_revision {
            return Err(ApiError::conflict("environment_revision_changed"));
        }
        let revision = request.expected_revision + 1;
        let recipe = serde_json::to_value(&request.recipe)
            .map_err(|_| ApiError::bad_request("invalid_environment_recipe"))?;
        let digest = format!(
            "{:x}",
            Sha256::digest(
                serde_json::to_vec(&request.recipe)
                    .map_err(|_| ApiError::bad_request("invalid_environment_recipe"))?
            )
        );
        sqlx::query("INSERT INTO cloud_environment_versions(owner_id,project_id,revision,source,recipe,digest,created_at) VALUES($1,$2,$3,'project',$4,$5,$6)").bind(owner).bind(project).bind(revision).bind(recipe).bind(&digest).bind(now).execute(&mut *tx).await?;
        self.enqueue_preparation_tx(
            &mut tx,
            owner,
            actor,
            actor_metadata,
            project,
            revision,
            &digest,
            &repository_identity,
            &requested_ref,
            now,
        )
        .await?;
        tx.commit().await?;
        crate::runtime::lifecycle::wakeup::send();
        Ok(EnvironmentVersion {
            project_id: project.into(),
            revision,
            source: EnvironmentSource::Project,
            recipe: request.recipe,
            created_at: now,
        })
    }
}

impl Store {
    pub fn with_computer_control(mut self, control: ComputerControl) -> Self {
        self.computers = Some(Arc::new(control));
        self
    }
    pub async fn computer_capabilities(
        &self,
        owner: &str,
        now: i64,
    ) -> Result<CloudCapabilities, ApiError> {
        let schema: bool =
            sqlx::query_scalar("SELECT to_regclass('cloud_managed_bases') IS NOT NULL")
                .fetch_one(&self.pool)
                .await?;
        let rows: Vec<(String, Value, Option<Value>)> = if schema {
            sqlx::query_as("SELECT DISTINCT ON(provider) provider,capabilities,to_jsonb(b)->'runtime_manifest' FROM cloud_managed_bases b WHERE state='verified' AND jsonb_typeof(to_jsonb(b)->'runtime_manifest')='object' AND (NOT $1 OR to_jsonb(b)->'runtime_manifest'->'manifest'->>'rustOrganizationExecutionVersion'='1') AND (provider<>'boxd' OR to_jsonb(b)->'runtime_manifest'->'manifest'->>'rustRuntimeSupervisorVersion'='1') ORDER BY provider,created_at DESC,release_id DESC").bind(owner.starts_with("organization:")).fetch_all(&self.pool).await?
        } else {
            vec![]
        };
        let mut providers = Vec::new();
        for id in ProviderId::ALL {
            let capability = rows
                .iter()
                .find(|(provider, _, _)| provider == id.as_str())
                .and_then(|(_, value, manifest)| {
                    let mut caps =
                        serde_json::from_value::<ProviderCapabilities>(value.clone()).ok()?;
                    let release: crate::managed_bases::ManagedBaseRelease =
                        serde_json::from_value(manifest.as_ref()?.clone()).ok()?;
                    release.validate().ok()?;
                    caps.command_admission_version =
                        release.manifest.rust_command_admission_version.filter(|_| {
                            release
                                .manifest
                                .command_admission
                                .as_ref()
                                .is_some_and(|receipt| receipt.valid())
                        });
                    Some(caps)
                })
                .filter(|c| c.id == id);
            providers.push(
                capability.unwrap_or(ProviderCapabilities {
                    id,
                    configured: false,
                    managed_base_ready: false,
                    command_admission_version: None,
                    preserves_disk: false,
                    preserves_memory: false,
                    supports_snapshots: false,
                    supports_docker: false,
                    sizes: vec![],
                    unavailable_reason: Some(
                        if schema {
                            "managed_base_unavailable"
                        } else {
                            "cloud_computer_schema_not_ready"
                        }
                        .into(),
                    ),
                }),
            );
        }
        let (candidate, candidate_unavailable) =
            self.staging_base_candidate_availability(owner, now).await?;
        if let Some(reason) = candidate_unavailable {
            if let Some(pin) = self
                .staging_candidate
                .as_ref()
                .filter(|p| p.owner_id == owner)
            {
                if let Some(provider) = providers.iter_mut().find(|p| p.id == pin.provider) {
                    provider.configured = false;
                    provider.managed_base_ready = false;
                    provider.command_admission_version = None;
                    provider.sizes.clear();
                    provider.unavailable_reason = Some(reason.into());
                }
            }
        }
        if let Some((_, c)) = &candidate {
            if let Some(provider) = providers.iter_mut().find(|p| p.id == c.provider) {
                *provider = c.capabilities.clone();
                provider.command_admission_version =
                    c.manifest.rust_command_admission_version.filter(|_| {
                        c.manifest
                            .command_admission
                            .as_ref()
                            .is_some_and(|receipt| {
                                receipt.valid()
                                    && receipt
                                        .runtime_sha256
                                        .eq_ignore_ascii_case(&c.runtime_sha256)
                            })
                    });
            }
        }
        let entitled:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM api_entitlements WHERE account_id=$1 AND kind='cloud-workspace' AND (status IN ('active','grace') OR (status='ended' AND paid_through>$2)) AND (paid_through IS NULL OR paid_through>$2))").bind(owner).bind(now).fetch_one(&self.pool).await?;
        let mut blockers = Vec::new();
        if let Some(reason) = candidate_unavailable {
            blockers.push(reason.into());
        }
        if !entitled
            && !candidate
                .as_ref()
                .is_some_and(|(p, _)| p.allow_staging_compute)
        {
            blockers.push("cloud_entitlement_required".into());
        }
        if !providers.iter().any(ProviderCapabilities::can_create) {
            blockers.push("managed_base_unavailable".into());
        }
        Ok(CloudCapabilities {
            staging_candidate: self
                .staging_candidate
                .as_ref()
                .filter(|p| p.owner_id == owner)
                .map(|p| StagingCandidate {
                    provider_id: p.provider,
                    release_id: p.release_id.clone(),
                    status: if candidate_unavailable.is_some() {
                        "unavailable"
                    } else {
                        "uncertified"
                    }
                    .into(),
                    expires_at: p.expires_at,
                }),
            can_configure: self
                .computers
                .as_ref()
                .is_some_and(|c| c.accounts.contains(owner)),
            can_allocate: blockers.is_empty(),
            allocation_blockers: blockers,
            providers,
        })
    }
    pub async fn project_environment(
        &self,
        owner: &str,
        project: &str,
    ) -> Result<EnvironmentVersion, ApiError> {
        let exists:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM api_cloud_projects WHERE account_id=$1 AND project_id=$2 AND included=true)").bind(owner).bind(project).fetch_one(&self.pool).await?;
        if !exists {
            return Err(ApiError::not_found("cloud_project_not_found"));
        }
        let row:Option<(i64,String,Value,i64)>=sqlx::query_as("SELECT revision,source,recipe,created_at FROM cloud_environment_versions WHERE owner_id=$1 AND project_id=$2 ORDER BY revision DESC LIMIT 1").bind(owner).bind(project).fetch_optional(&self.pool).await?;
        match row {
            None => Ok(EnvironmentVersion {
                project_id: project.into(),
                revision: 0,
                source: EnvironmentSource::Project,
                recipe: EnvironmentRecipe::default(),
                created_at: 0,
            }),
            Some((revision, source, recipe, created_at)) => Ok(EnvironmentVersion {
                project_id: project.into(),
                revision,
                source: match source.as_str() {
                    "project" => EnvironmentSource::Project,
                    "repository" => EnvironmentSource::Repository,
                    _ => return Err(ApiError::unavailable("environment_record_invalid")),
                },
                recipe: serde_json::from_value(recipe)
                    .map_err(|_| ApiError::unavailable("environment_record_invalid"))?,
                created_at,
            }),
        }
    }
    pub async fn save_project_environment_scoped(
        &self,
        access: &crate::access::Access,
        project: &str,
        request: SaveEnvironment,
        now: i64,
    ) -> Result<EnvironmentVersion, ApiError> {
        self.computers
            .as_ref()
            .ok_or_else(|| ApiError::unavailable("cloud_environment_writes_disabled"))?
            .save_environment_scoped(access, project, request, now)
            .await
    }
    pub async fn save_project_environment(
        &self,
        owner: &str,
        project: &str,
        request: SaveEnvironment,
        now: i64,
    ) -> Result<EnvironmentVersion, ApiError> {
        self.computers
            .as_ref()
            .ok_or_else(|| ApiError::unavailable("cloud_environment_writes_disabled"))?
            .save_environment(owner, project, request, now)
            .await
    }
}
