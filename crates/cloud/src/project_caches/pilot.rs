//! Operator-configured staging mechanism rehearsal. This authority never changes
//! a managed candidate into a verified base or publishes a cache certificate.
use super::*;
use crate::managed_bases::candidate::{ManagedBaseCandidate, StagingBasePin};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use zuse_protocol::cloud_computers::ProviderId;

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Input {
    pub recipe_digest: String,
    pub recipe_source: String,
    pub source_commit: String,
    pub dependency_digest: String,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Pilot {
    pub version: u32,
    pub pilot_id: String,
    pub owner_id: String,
    pub actor_id: String,
    pub project_id: String,
    pub repository_identity: String,
    pub candidate_sha256: String,
    pub base_release_id: String,
    pub base_artifact_id: String,
    pub base_artifact_version: String,
    pub runtime_sha256: String,
    pub architecture: String,
    pub expires_at: i64,
    pub protected_workspace_id: String,
    pub protected_machine_id: String,
    pub max_active_machines: u32,
    pub max_owned_machines: u32,
    pub inputs: Vec<Input>,
    /// A measured CLI receipt, explicitly not an operator certificate.
    pub mechanism_capability: CacheCapabilityReceipt,
}
pub(crate) struct Authority {
    pub pilot: Pilot,
    pub pin: Arc<StagingBasePin>,
}
fn denied() -> ApiError {
    ApiError::forbidden("staging_project_cache_pilot_fenced")
}
fn hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
impl Pilot {
    pub fn fingerprint(&self) -> Result<String, ApiError> {
        Ok(format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(self).map_err(|_| denied())?)
        ))
    }
    pub(crate) fn validate(
        &self,
        pin: &StagingBasePin,
        environment: &str,
        now: i64,
    ) -> Result<(), ApiError> {
        if environment != "staging"
            || self.version != 1
            || self.owner_id.starts_with("organization:")
            || self.owner_id != self.actor_id
            || self.owner_id != pin.owner_id
            || pin.provider != ProviderId::Boxd
            || self.base_release_id != pin.release_id
            || self.base_artifact_id != pin.artifact_id
            || self.candidate_sha256 != pin.candidate_sha256
            || self.expires_at <= now
            || self.expires_at > pin.expires_at
            || self.expires_at > now + 1_800_000
            || self.protected_workspace_id.is_empty()
            || self.protected_machine_id.is_empty()
            || self.protected_workspace_id.len() > 160
            || self.protected_machine_id.len() > 160
            || self.max_active_machines != 2
            || self.max_owned_machines != 1
            || !hash(&self.runtime_sha256)
            || !hash(&self.candidate_sha256)
            || !matches!(self.architecture.as_str(), "x86_64" | "aarch64")
            || self.base_artifact_version.parse::<u64>().is_err()
            || self.base_artifact_version == "0"
            || self.pilot_id.is_empty()
            || self.pilot_id.len() > 128
            || !self
                .pilot_id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
            || self.project_id.is_empty()
            || self.project_id.len() > 128
            || self.inputs.is_empty()
            || self.inputs.len() > 2
            || !self.mechanism_capability.valid(&self.runtime_sha256)
        {
            return Err(denied());
        }
        pin.require_actor(&self.actor_id)?;
        if self.inputs.iter().any(|input| {
            !hash(&input.recipe_digest)
                || !hash(&input.dependency_digest)
                || !matches!(input.recipe_source.as_str(), "project" | "repository")
                || !matches!(input.source_commit.len(), 40 | 64)
                || !input
                    .source_commit
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        }) {
            return Err(denied());
        }
        Ok(())
    }
    pub fn permits(&self, identity: &CacheIdentity) -> Result<bool, ApiError> {
        Ok(
            identity.staging_pilot_sha256.as_deref() == Some(self.fingerprint()?.as_str())
                && identity.owner_id == self.owner_id
                && identity.project_id == self.project_id
                && identity.repository_identity == self.repository_identity
                && identity.provider == ProviderId::Boxd
                && identity.base_release_id == self.base_release_id
                && identity.base_artifact_id == self.base_artifact_id
                && identity.base_artifact_version.as_deref()
                    == Some(self.base_artifact_version.as_str())
                && identity.base_runtime_sha256 == self.runtime_sha256
                && identity.architecture == self.architecture
                && self.inputs.iter().any(|input| {
                    input.recipe_digest == identity.recipe_digest
                        && input.recipe_source == identity.recipe_source
                        && input.source_commit == identity.source_commit
                        && input.dependency_digest == identity.dependency_digest()
                }),
        )
    }
    pub fn source_allowed(
        &self,
        owner: &str,
        project: &str,
        source: &super::ref_resolution::ResolvedSource,
    ) -> Result<(), ApiError> {
        let digest = format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(&source.recipe).map_err(|_| denied())?)
        );
        if owner != self.owner_id
            || project != self.project_id
            || !self.inputs.iter().any(|input| {
                input.source_commit == source.commit
                    && input.recipe_source == source.recipe_source
                    && input.recipe_digest == digest
            })
        {
            return Err(denied());
        }
        Ok(())
    }
}
/// Read only trusted process configuration. No request field can create authority.
/// Parsing is repeated at boundaries so revocation/restart/expiry cannot retain a
/// cached positive authorization. Cleanup does not call this function.
pub(crate) fn current(now: i64) -> Result<Option<Authority>, ApiError> {
    let Some(value) = std::env::var("ZUSE_CLOUD_STAGING_PROJECT_CACHE_PILOT")
        .ok()
        .filter(|s| !s.is_empty())
    else {
        return Ok(None);
    };
    if value.len() > 16384 {
        return Err(denied());
    }
    let pilot: Pilot = serde_json::from_str(&value).map_err(|_| denied())?;
    let creation: Vec<String> = std::env::var("ZUSE_CLOUD_CREATE_ACCOUNTS")
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect();
    let pin = StagingBasePin::from_environment(&creation)
        .map_err(|_| denied())?
        .ok_or_else(denied)?;
    pilot.validate(
        &pin,
        &std::env::var("ZUSE_DEPLOYMENT_ENVIRONMENT").unwrap_or_default(),
        now,
    )?;
    Ok(Some(Authority { pilot, pin }))
}
impl Authority {
    pub(crate) async fn candidate_tx(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        now: i64,
    ) -> Result<ManagedBaseCandidate, ApiError> {
        let candidate = self.pin.load(tx, now, true).await?;
        if candidate.runtime_sha256 != self.pilot.runtime_sha256
            || candidate.architecture != self.pilot.architecture
            || candidate
                .initial_runtime_bootstrap
                .as_ref()
                .is_none_or(|spec| {
                    !spec.valid(ProviderId::Boxd)
                        || spec.runtime_sha256 != self.pilot.runtime_sha256
                })
        {
            return Err(denied());
        }
        Ok(candidate)
    }
    pub(crate) async fn request_tx(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        request: &worker::Request,
        now: i64,
    ) -> Result<(), ApiError> {
        if request.owner != self.pilot.owner_id
            || request.actor_id != self.pilot.actor_id
            || !request.actor.is_null()
            || request.project != self.pilot.project_id
            || request.repository_identity != self.pilot.repository_identity
        {
            return Err(denied());
        }
        self.candidate_tx(tx, now).await?;
        let included:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM api_cloud_projects WHERE account_id=$1 AND project_id=$2 AND included=true AND state='connected' AND repository_identity=$3 AND secret_bindings='[]'::jsonb)").bind(&request.owner).bind(&request.project).bind(&request.repository_identity).fetch_one(&mut **tx).await?;
        if !included {
            return Err(denied());
        }
        Ok(())
    }
}
pub(crate) async fn authorized_tx(
    tx: &mut Transaction<'_, Postgres>,
    identity: &CacheIdentity,
    now: i64,
) -> Result<bool, ApiError> {
    if identity.staging_pilot_sha256.is_none() {
        return Ok(false);
    }
    let Some(authority) = current(now)? else {
        return Ok(false);
    };
    if !authority.pilot.permits(identity)? {
        return Ok(false);
    }
    authority.candidate_tx(tx, now).await?;
    let included:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM api_cloud_projects WHERE account_id=$1 AND project_id=$2 AND included=true AND state='connected' AND repository_identity=$3 AND secret_bindings='[]'::jsonb)").bind(&identity.owner_id).bind(&identity.project_id).bind(&identity.repository_identity).fetch_one(&mut **tx).await?;
    Ok(included)
}
/// A durable provider reservation survives a lost create reply. Count every
/// tracked provider and pending task, not just ready computers for one account.
/// The exclusive Root lane additionally verifies no untracked provider resource.
pub(crate) async fn capacity_tx(
    tx: &mut Transaction<'_, Postgres>,
    own_operation: Option<&str>,
) -> Result<(), ApiError> {
    let authority = current(chrono::Utc::now().timestamp_millis())?.ok_or_else(denied)?;
    let protected:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM api_cloud_workspaces WHERE workspace_id=$1 AND provider='boxd' AND provider_sandbox_id=$2 AND request_config->>'runtimeGeneration'='1' AND state NOT IN('deleting','deleted'))")
        .bind(&authority.pilot.protected_workspace_id).bind(&authority.pilot.protected_machine_id).fetch_one(&mut **tx).await?;
    if !protected {
        return Err(denied());
    }
    sqlx::query(
        "SELECT pg_advisory_xact_lock(hashtextextended('staging-project-cache-global-cap',0))",
    )
    .execute(&mut **tx)
    .await?;
    let count:i64=sqlx::query_scalar("SELECT (SELECT count(*) FROM (SELECT provider,provider_sandbox_id AS id FROM api_cloud_workspaces WHERE state<>'deleted' AND provider_sandbox_id IS NOT NULL UNION SELECT provider,resource_id FROM zuse_rust_project_cache_resources WHERE resource_kind IN('builder','verifier') AND absent_observed_at IS NULL) v)+(SELECT count(*) FROM api_cloud_workspaces WHERE state IN('queued','provisioning','setup','resuming') AND provider_sandbox_id IS NULL)+(SELECT count(*) FROM zuse_rust_project_cache_operations o WHERE o.operation_id IS DISTINCT FROM $1 AND o.state IN('running','uncertain') AND o.phase IN('allocate','verify') AND o.resource_receipts->'effects'->o.phase->>'requested'='true' AND (CASE WHEN o.phase='allocate' THEN o.resource_receipts#>>'{provider,builderId}' ELSE o.resource_receipts#>>'{provider,verifierId}' END) IS NULL)").bind(own_operation).fetch_one(&mut **tx).await?;
    if count >= 2 {
        return Err(ApiError::conflict("staging_project_cache_resource_limit"));
    }
    Ok(())
}
