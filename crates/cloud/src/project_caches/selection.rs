//! Select only an exact, verified environment after the remote commit is resolved.
//! Resume never calls this lookup. Missing capability/cache uses the managed base.
use super::*;
use crate::providers::artifact::ArtifactBinding;

#[derive(Clone)]
pub struct CacheHit {
    pub cache_key: String,
    pub identity: CacheIdentity,
    pub artifact_id: String,
    pub artifact_version: Option<String>,
    pub artifact_binding: Option<ArtifactBinding>,
    pub receipt: PreparationReceipt,
}
impl CacheHit {
    pub fn config(&self) -> Value {
        json!({"version":1,"identity":self.identity,"cacheKey":self.cache_key,"artifactId":self.artifact_id,"artifactVersion":self.artifact_version,"providerArtifactBinding":self.artifact_binding,"receipt":self.receipt})
    }
}

pub async fn lookup_tx(
    tx: &mut Transaction<'_, Postgres>,
    identity: &CacheIdentity,
) -> Result<Option<CacheHit>, ApiError> {
    let key = identity.key()?;
    if identity.staging_pilot_sha256.is_some() {
        if !super::pilot::authorized_tx(tx, identity, chrono::Utc::now().timestamp_millis())
            .await
            .unwrap_or(false)
        {
            return Ok(None);
        }
    } else {
        let base: Option<Value> = sqlx::query_scalar(
            "SELECT to_jsonb(b) FROM zuse_rust_managed_base_admission_lock($1,$2) b",
        )
        .bind(identity.provider.as_str())
        .bind(&identity.base_release_id)
        .fetch_optional(&mut **tx)
        .await?;
        let Some(base) = base else { return Ok(None) };
        if base["state"] != "verified" || base["artifact_id"] != identity.base_artifact_id {
            return Ok(None);
        }
        let spec =
            serde_json::from_value::<crate::managed_bases::bootstrap::InitialRuntimeBootstrap>(
                base["runtime_manifest"]["initialRuntimeBootstrap"].clone(),
            )
            .ok();
        if spec.as_ref().is_none_or(|s| {
            !s.valid(identity.provider) || s.runtime_sha256 != identity.base_runtime_sha256
        }) {
            return Ok(None);
        }
        let certified:Option<(String,String,Value)>=sqlx::query_as("SELECT c.runtime_sha256,c.architecture,c.receipt FROM zuse_rust_project_cache_capability_lock($1,$2) c WHERE c.state='certified'")
        .bind(identity.provider.as_str()).bind(&identity.base_release_id).fetch_optional(&mut **tx).await?;
        let Some((runtime, architecture, receipt)) = certified else {
            return Ok(None);
        };
        let capability: CacheCapabilityReceipt = serde_json::from_value(receipt)
            .map_err(|_| ApiError::unavailable("project_cache_capability_invalid"))?;
        if architecture != identity.architecture
            || runtime != identity.base_runtime_sha256
            || !capability.valid(&runtime)
        {
            return Ok(None);
        }
    }
    let row:Option<Value>=sqlx::query_scalar("SELECT to_jsonb(c) FROM zuse_rust_project_cache_entries c WHERE cache_key=$1 AND owner_id=$2 AND project_id=$3 AND provider=$4 AND base_release_id=$5 AND state='ready' AND revoked_at IS NULL FOR SHARE")
        .bind(&key).bind(&identity.owner_id).bind(&identity.project_id).bind(identity.provider.as_str()).bind(&identity.base_release_id).fetch_optional(&mut **tx).await?;
    let Some(row) = row else { return Ok(None) };
    if row["identity"] != json!(identity) {
        return Err(ApiError::unavailable("project_cache_identity_conflict"));
    }
    let operation = row["preparation_operation_id"]
        .as_str()
        .ok_or_else(|| ApiError::unavailable("project_cache_receipt_invalid"))?;
    let receipt: PreparationReceipt = serde_json::from_value(row["receipt"].clone())
        .map_err(|_| ApiError::unavailable("project_cache_receipt_invalid"))?;
    receipt.validate(identity, operation)?;
    let artifact = row["artifact_id"]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| ApiError::unavailable("project_cache_receipt_invalid"))?;
    let binding: Option<ArtifactBinding> = serde_json::from_value(row["artifact_binding"].clone())
        .map_err(|_| ApiError::unavailable("project_cache_artifact_binding_invalid"))?;
    if let Some(binding) = &binding {
        binding
            .validate(identity.provider, artifact)
            .map_err(|_| ApiError::unavailable("project_cache_artifact_binding_invalid"))?;
        if binding.artifact_id != artifact
            || row["artifact_version"].as_str() != Some(binding.artifact_version.as_str())
            || binding.runtime_sha256 != identity.base_runtime_sha256
        {
            return Err(ApiError::unavailable(
                "project_cache_artifact_binding_invalid",
            ));
        }
    } else if identity.provider == zuse_protocol::cloud_computers::ProviderId::E2b {
        return Ok(None);
    }
    Ok(Some(CacheHit {
        cache_key: key,
        identity: identity.clone(),
        artifact_id: artifact.into(),
        artifact_version: row["artifact_version"].as_str().map(str::to_owned),
        artifact_binding: binding,
        receipt,
    }))
}

/// Exact SHA requests need no mutable-ref resolution. Named refs deliberately
/// miss until the allocation resolver can pin their current remote commit.
/// Invalid, stale or unavailable artifacts remain an optimization miss.
pub(crate) async fn for_new_task_tx(
    tx: &mut Transaction<'_, Postgres>,
    owner: &str,
    project: &str,
    repository: &str,
    provider: &str,
    release: &str,
    base: &str,
    source_commit: &str,
    recipe: &Value,
) -> Result<Option<CacheHit>, ApiError> {
    for_resolved_task_tx(
        tx,
        owner,
        project,
        repository,
        provider,
        release,
        base,
        source_commit,
        recipe,
        "project",
    )
    .await
}
pub(crate) async fn for_resolved_task_tx(
    tx: &mut Transaction<'_, Postgres>,
    owner: &str,
    project: &str,
    repository: &str,
    provider: &str,
    release: &str,
    base: &str,
    source_commit: &str,
    recipe: &Value,
    recipe_source: &str,
) -> Result<Option<CacheHit>, ApiError> {
    if !matches!(source_commit.len(), 40 | 64)
        || !source_commit
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    {
        return Ok(None);
    }
    let recipe: zuse_protocol::cloud_computers::EnvironmentRecipe =
        serde_json::from_value(recipe.clone())
            .map_err(|_| ApiError::conflict("environment_recipe_invalid"))?;
    if !recipe.secret_bindings.is_empty() {
        return Ok(None);
    }
    use sha2::Digest;
    let digest = format!(
        "{:x}",
        sha2::Sha256::digest(
            serde_json::to_vec(&recipe)
                .map_err(|_| ApiError::conflict("environment_recipe_invalid"))?
        )
    );
    let candidates:Vec<Value>=sqlx::query_scalar("SELECT identity FROM zuse_rust_project_cache_entries WHERE owner_id=$1 AND project_id=$2 AND provider=$3 AND base_release_id=$4 AND base_artifact_id=$5 AND source_commit=$6 AND recipe_digest=$7 AND state='ready' AND revoked_at IS NULL ORDER BY verified_at DESC,cache_key LIMIT 4")
        .bind(owner).bind(project).bind(provider).bind(release).bind(base).bind(source_commit).bind(digest).fetch_all(&mut **tx).await?;
    for value in candidates {
        let Ok(identity) = serde_json::from_value::<CacheIdentity>(value) else {
            continue;
        };
        if identity.repository_identity != repository || identity.recipe_source != recipe_source {
            continue;
        }
        if let Some(hit) = lookup_tx(tx, &identity).await? {
            return Ok(Some(hit));
        }
    }
    Ok(None)
}
