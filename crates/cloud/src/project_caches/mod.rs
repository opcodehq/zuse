//! Optional prepared project artifacts. A task always retains managed-base fallback.
//! Preparation intents never contain an agent connection, task disk, or runtime grant.
mod allocation;
pub(crate) mod backends;
pub(crate) mod boxd;
pub mod identity;
pub(crate) mod pilot;
pub(crate) mod selection;
pub(crate) mod source;
pub mod user_images;
pub use selection::{CacheHit, lookup_tx};
mod requests;
pub mod worker;

/// Per-project snapshots are off by default: every task starts from the one
/// shared base image and clones only its own repository. Operators opt in
/// with `ZUSE_CLOUD_PROJECT_CACHES=1` where provider snapshots are affordable.
pub(crate) fn enabled() -> bool {
    // The crate's cache tests exercise the opt-in path itself.
    cfg!(test) || std::env::var("ZUSE_CLOUD_PROJECT_CACHES").is_ok_and(|v| v == "1")
}
use crate::error::ApiError;
pub use identity::CacheIdentity;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::{PgPool, Postgres, Transaction};

pub const MIGRATION: &str = include_str!("../../migrations/0028_project_environment_caches.sql");
pub const FORMAT_VERSION: u32 = 1;

pub(crate) fn require_runtime_capability(
    config: &Value,
    capabilities: Option<&Vec<String>>,
) -> Result<(), ApiError> {
    if config.get("rustProjectCache").is_some_and(|v| !v.is_null())
        && capabilities.is_none_or(|caps| {
            !caps
                .iter()
                .any(|c| c == zuse_cloud_runtime::project_cache::CAPABILITY)
        })
    {
        return Err(ApiError::conflict("project_cache_runtime_update_required"));
    }
    Ok(())
}

pub(crate) async fn schema_ready(pool: &PgPool) -> Result<bool, ApiError> {
    Ok(sqlx::query_scalar("SELECT to_regclass('zuse_rust_project_preparation_requests') IS NOT NULL AND to_regclass('zuse_rust_project_cache_entries') IS NOT NULL AND to_regclass('cloud_project_cache_capabilities') IS NOT NULL AND to_regclass('zuse_rust_project_cache_operations') IS NOT NULL AND to_regclass('zuse_rust_project_source_bundles') IS NOT NULL AND to_regclass('zuse_rust_project_preparation_keys') IS NOT NULL AND to_regclass('zuse_rust_project_cache_resources') IS NOT NULL AND to_regprocedure('zuse_rust_project_cache_capability_lock(text,text)') IS NOT NULL").fetch_one(pool).await?)
}

pub(crate) async fn schema_ready_tx(tx: &mut Transaction<'_, Postgres>) -> Result<bool, ApiError> {
    Ok(sqlx::query_scalar("SELECT to_regclass('zuse_rust_project_preparation_requests') IS NOT NULL AND to_regclass('zuse_rust_project_cache_entries') IS NOT NULL AND to_regclass('cloud_project_cache_capabilities') IS NOT NULL AND to_regclass('zuse_rust_project_cache_operations') IS NOT NULL AND to_regclass('zuse_rust_project_source_bundles') IS NOT NULL AND to_regclass('zuse_rust_project_preparation_keys') IS NOT NULL AND to_regclass('zuse_rust_project_cache_resources') IS NOT NULL AND to_regprocedure('zuse_rust_project_cache_capability_lock(text,text)') IS NOT NULL").fetch_one(&mut **tx).await?)
}

/// Operator certification is separate from ordinary harness/runtime support.
/// Existing managed images have no row and therefore cannot use a cache hit.
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CacheCapabilityReceipt {
    pub version: u32,
    pub runtime_sha256: String,
    pub environment_cache_version: u32,
    pub secret_free_source_bundle_version: u32,
    pub retained_task_storage_absent_required: bool,
    pub credential_free_preparation_required: bool,
    #[serde(default)]
    pub preparation_capture_version: u32,
}
impl CacheCapabilityReceipt {
    pub fn valid(&self, runtime: &str) -> bool {
        self.version == 1
            && self.environment_cache_version == 1
            && self.secret_free_source_bundle_version == 1
            && self.retained_task_storage_absent_required
            && self.credential_free_preparation_required
            && self.preparation_capture_version == 1
            && self.runtime_sha256 == runtime
            && runtime.len() == 64
            && runtime
                .bytes()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PreparationReceipt {
    pub version: u32,
    pub cache_key: String,
    pub operation_id: String,
    pub runtime_sha256: String,
    pub recipe_digest: String,
    pub dependency_digest: String,
    pub source_commit: String,
    pub source_bundle_sha256: String,
    pub credentials_removed: bool,
    pub task_storage_absent: bool,
    pub agent_profiles_absent: bool,
    pub secret_free_source: bool,
}
impl PreparationReceipt {
    pub fn validate(&self, identity: &CacheIdentity, operation: &str) -> Result<(), ApiError> {
        if self.version != FORMAT_VERSION
            || self.operation_id != operation
            || self.cache_key != identity.key()?
            || self.runtime_sha256 != identity.base_runtime_sha256
            || self.recipe_digest != identity.recipe_digest
            || self.dependency_digest != identity.dependency_digest()
            || self.source_commit != identity.source_commit
            || !self.credentials_removed
            || !self.task_storage_absent
            || !self.agent_profiles_absent
            || !self.secret_free_source
            || self.source_bundle_sha256.len() != 64
            || !self
                .source_bundle_sha256
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        {
            return Err(ApiError::conflict(
                "project_cache_preparation_receipt_invalid",
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;

pub(crate) mod ref_resolution;

mod allocation_ref;

#[cfg(test)]
mod allocation_ref_tests;

mod retirement;

#[cfg(test)]
mod private_source_tests;

#[cfg(test)]
mod pilot_tests;
