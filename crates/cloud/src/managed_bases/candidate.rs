//! Owner-only staging acceptance. Candidate provenance is never certification.
mod admission;
use super::*;
use sha2::{Digest, Sha256};
use std::sync::Arc;
mod actor;
mod retained;
pub use retained::RetainedWorkspaceBinding;

pub const MIGRATION: &str =
    include_str!("../../migrations/0020_staging_managed_base_candidates.sql");
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ManagedBaseCandidate {
    pub provider: ProviderId,
    pub release_id: String,
    pub artifact_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_artifact_binding: Option<crate::providers::artifact::ArtifactBinding>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initial_runtime_bootstrap: Option<super::bootstrap::InitialRuntimeBootstrap>,
    pub architecture: String,
    pub capabilities: ProviderCapabilities,
    pub manifest: ImageManifest,
    pub harnesses: Vec<PinnedHarness>,
    pub harness_release: String,
    pub runtime_sha256: String,
    pub provenance_report_sha256: String,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StagingBasePin {
    pub owner_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor_id: Option<String>,
    pub provider: ProviderId,
    pub release_id: String,
    pub artifact_id: String,
    pub harness_release: String,
    pub manifest_sha256: String,
    pub candidate_sha256: String,
    pub expires_at: i64,
    #[serde(default)]
    pub allow_staging_compute: bool,
    /// Explicit owner-only staging capacity; omitted pins keep the pilot default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_computers: Option<u16>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub unlimited_computers: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub retained_workspaces: Vec<RetainedWorkspaceBinding>,
}
fn digest<T: Serialize>(value: &T) -> Result<String, ApiError> {
    fn sorted(v: Value) -> Value {
        match v {
            Value::Object(m) => Value::Object(
                m.into_iter()
                    .map(|(k, v)| (k, sorted(v)))
                    .collect::<std::collections::BTreeMap<_, _>>()
                    .into_iter()
                    .collect(),
            ),
            Value::Array(a) => Value::Array(a.into_iter().map(sorted).collect()),
            v => v,
        }
    }
    let bytes = serde_json::to_vec(&sorted(serde_json::to_value(value).map_err(|_| invalid())?))
        .map_err(|_| invalid())?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}
fn invalid() -> ApiError {
    ApiError::bad_request("staging_managed_base_candidate_invalid")
}
impl ManagedBaseCandidate {
    pub fn validate(&self) -> Result<(), ApiError> {
        if let Some(spec) = &self.initial_runtime_bootstrap {
            spec.validate(self.provider, &self.manifest)?;
            if spec.runtime_sha256 != self.runtime_sha256 {
                return Err(invalid());
            }
        }
        validate_provider_binding(
            self.provider,
            &self.artifact_id,
            self.provider_artifact_binding.as_ref(),
        )?;
        if self
            .provider_artifact_binding
            .as_ref()
            .is_some_and(|v| v.runtime_sha256 != self.runtime_sha256)
        {
            return Err(invalid());
        }
        validate_content(
            self.provider,
            &self.manifest.build_id,
            &self.artifact_id,
            &self.architecture,
            &self.capabilities,
            &self.manifest,
            &self.harnesses,
        )?;
        if !identifier(&self.release_id)
            || !sha256(&self.harness_release)
            || !sha256(&self.runtime_sha256)
            || !sha256(&self.provenance_report_sha256)
            || self.manifest.rust_runtime_supervisor_version != Some(1)
            || self
                .manifest
                .command_admission
                .as_ref()
                .is_some_and(|receipt| {
                    !receipt
                        .runtime_sha256
                        .eq_ignore_ascii_case(&self.runtime_sha256)
                })
        {
            return Err(invalid());
        }
        Ok(())
    }
    pub fn pin(
        &self,
        owner: &str,
        expires_at: i64,
        allow_staging_compute: bool,
    ) -> Result<StagingBasePin, ApiError> {
        self.validate()?;
        let pin = StagingBasePin {
            owner_id: owner.into(),
            actor_id: None,
            provider: self.provider,
            release_id: self.release_id.clone(),
            artifact_id: self.artifact_id.clone(),
            harness_release: self.harness_release.clone(),
            manifest_sha256: digest(&self.manifest)?,
            candidate_sha256: digest(self)?,
            expires_at,
            allow_staging_compute,
            max_computers: None,
            unlimited_computers: false,
            retained_workspaces: Vec::new(),
        };
        pin.validate()?;
        Ok(pin)
    }
    pub async fn stage_url(
        &self,
        url: &str,
        owner: &str,
        ttl_seconds: i64,
        allow_staging_compute: bool,
        now: i64,
    ) -> Result<StagingBasePin, ApiError> {
        if std::env::var("ZUSE_DEPLOYMENT_ENVIRONMENT").as_deref() != Ok("staging")
            || !(60..=86400).contains(&ttl_seconds)
        {
            return Err(ApiError::forbidden("staging_candidate_deployment_required"));
        }
        let pin = self.pin(owner, now + ttl_seconds * 1000, allow_staging_compute)?;
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .connect(url)
            .await
            .map_err(|_| ApiError::unavailable("managed_base_database_unavailable"))?;
        self.stage(&pool, &pin, now).await?;
        Ok(pin)
    }
    pub(crate) async fn stage(
        &self,
        pool: &PgPool,
        pin: &StagingBasePin,
        now: i64,
    ) -> Result<(), ApiError> {
        self.validate()?;
        pin.matches(self, now)?;
        let value = serde_json::to_value(self).map_err(|_| invalid())?;
        let mut tx = pool.begin().await?;
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
            .bind(format!(
                "managed-base:{}:{}",
                self.provider.as_str(),
                self.release_id
            ))
            .execute(&mut *tx)
            .await?;
        let prior:Option<(String,Option<Value>)>=sqlx::query_as("SELECT state,candidate_manifest FROM cloud_managed_bases WHERE provider=$1 AND release_id=$2 FOR UPDATE").bind(self.provider.as_str()).bind(&self.release_id).fetch_optional(&mut *tx).await?;
        if let Some((state, record)) = prior {
            if state != "preparing" || record.as_ref() != Some(&value) {
                return Err(ApiError::conflict("managed_base_release_immutable"));
            }
        } else {
            sqlx::query("INSERT INTO cloud_managed_bases(provider,release_id,artifact_id,architecture,capabilities,state,created_at,candidate_manifest) VALUES($1,$2,$3,$4,$5,'preparing',$6,$7)").bind(self.provider.as_str()).bind(&self.release_id).bind(&self.artifact_id).bind(&self.architecture).bind(json!(self.capabilities)).bind(now).bind(value).execute(&mut *tx).await?;
        }
        tx.commit().await?;
        Ok(())
    }
    pub(crate) fn settings(&self, pin: &StagingBasePin) -> Value {
        let mut settings = manifest_settings(
            self.provider,
            &self.release_id,
            &self.capabilities,
            &self.manifest,
        );
        if let Some(binding) = &self.provider_artifact_binding {
            settings["providerArtifactBinding"] = json!(binding);
        }
        if let Some(spec) = &self.initial_runtime_bootstrap {
            settings["initialRuntimeBootstrap"] = json!(spec);
        }
        settings["managedCandidateSha256"] = json!(pin.candidate_sha256);
        settings["managedCandidateArtifactId"] = json!(pin.artifact_id);
        settings["managedBaseReadiness"] = json!("uncertified-staging");
        settings
    }
}
impl StagingBasePin {
    /// An explicit owner-only staging override can remove the pilot count cap.
    /// Normal entitlement admission is unchanged.
    pub(crate) fn computer_limit(&self) -> Option<i64> {
        if self.allow_staging_compute && self.unlimited_computers {
            None
        } else if self.allow_staging_compute {
            Some(i64::from(self.max_computers.unwrap_or(2)))
        } else {
            Some(1)
        }
    }
    fn validate(&self) -> Result<(), ApiError> {
        if self.unlimited_computers && (!self.allow_staging_compute || self.max_computers.is_some())
        {
            return Err(invalid());
        }
        if self
            .max_computers
            .is_some_and(|limit| !self.allow_staging_compute || !(2..=10).contains(&limit))
        {
            return Err(invalid());
        }
        self.validate_scope()?;
        self.validate_retained()?;
        if !identifier(&self.release_id)
            || !identifier(&self.artifact_id)
            || !sha256(&self.harness_release)
            || !sha256(&self.manifest_sha256)
            || !sha256(&self.candidate_sha256)
        {
            return Err(invalid());
        }
        Ok(())
    }
    pub fn from_environment(creation_accounts: &[String]) -> anyhow::Result<Option<Arc<Self>>> {
        let Some(value) = std::env::var("ZUSE_CLOUD_STAGING_BASE_PIN")
            .ok()
            .filter(|v| !v.is_empty())
        else {
            return Ok(None);
        };
        anyhow::ensure!(value.len() <= 16384, "Staging candidate pin too large");
        let pin: Self = serde_json::from_str(&value)
            .map_err(|_| anyhow::anyhow!("Invalid staging candidate pin"))?;
        let auth: Vec<String> = std::env::var("ZUSE_CLOUD_AUTH_WRITE_ACCOUNTS")
            .unwrap_or_default()
            .split(',')
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(str::to_owned)
            .collect();
        let now = chrono::Utc::now().timestamp_millis();
        // An expired pin no longer offers its candidate; it must not stop the
        // API from starting (a routine restart after expiry took staging down).
        if pin.expires_at <= now {
            tracing::warn!(
                provider = pin.provider.as_str(),
                "Staging candidate pin expired; starting without it"
            );
            return Ok(None);
        }
        pin.deployment(
            &std::env::var("ZUSE_DEPLOYMENT_ENVIRONMENT").unwrap_or_default(),
            creation_accounts,
            &auth,
            now,
        )
        .map_err(|e| anyhow::anyhow!("{}", e.code))?;
        Ok(Some(Arc::new(pin)))
    }
    pub(crate) fn deployment(
        &self,
        environment: &str,
        creation: &[String],
        auth: &[String],
        now: i64,
    ) -> Result<(), ApiError> {
        self.validate()?;
        if environment != "staging"
            || creation != [self.owner_id.clone()]
            || !self.auth_cohort_matches(auth)
            || self.expires_at <= now
            || self.expires_at > now + 86400000
        {
            return Err(ApiError::forbidden(
                "staging_candidate_owner_cohort_required",
            ));
        }
        Ok(())
    }
    pub(crate) fn applies(&self, owner: &str, provider: &str) -> bool {
        self.owner_id == owner && self.provider.as_str() == provider
    }
    pub(crate) fn matches(&self, c: &ManagedBaseCandidate, now: i64) -> Result<(), ApiError> {
        self.validate()?;
        if self.owner_id.starts_with("organization:")
            && c.manifest.rust_organization_execution_version != Some(1)
        {
            return Err(ApiError::conflict(
                "managed_base_organization_runtime_required",
            ));
        }
        if self.expires_at <= now {
            return Err(ApiError::conflict("staging_managed_base_candidate_expired"));
        }
        if self.provider != c.provider
            || self.release_id != c.release_id
            || self.artifact_id != c.artifact_id
            || self.harness_release != c.harness_release
            || self.manifest_sha256 != digest(&c.manifest)?
            || self.candidate_sha256 != digest(c)?
        {
            return Err(ApiError::conflict("staging_managed_base_candidate_fenced"));
        }
        Ok(())
    }
    pub(crate) async fn load(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        now: i64,
        lock: bool,
    ) -> Result<ManagedBaseCandidate, ApiError> {
        let schema:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM information_schema.columns WHERE table_schema=current_schema() AND table_name='cloud_managed_bases' AND column_name='candidate_manifest')").fetch_one(&mut **tx).await?;
        if !schema {
            return Err(ApiError::conflict(
                "staging_managed_base_candidate_schema_required",
            ));
        }
        let row: Option<(String, String, Value, Value)> = if lock {
            super::admission_lock::record(tx, self.provider.as_str(), &self.release_id)
                .await?
                .filter(|r| {
                    r["state"] == "preparing"
                        && r["runtime_manifest"].is_null()
                        && !r["candidate_manifest"].is_null()
                })
                .map(|r| {
                    Ok::<_, ApiError>((
                        r["artifact_id"].as_str().ok_or_else(invalid)?.to_owned(),
                        r["architecture"].as_str().ok_or_else(invalid)?.to_owned(),
                        r["capabilities"].clone(),
                        r["candidate_manifest"].clone(),
                    ))
                })
                .transpose()?
        } else {
            sqlx::query_as("SELECT artifact_id,architecture,capabilities,candidate_manifest FROM cloud_managed_bases WHERE provider=$1 AND release_id=$2 AND state='preparing' AND runtime_manifest IS NULL AND candidate_manifest IS NOT NULL").bind(self.provider.as_str()).bind(&self.release_id).fetch_optional(&mut **tx).await?
        };
        let (artifact, arch, caps, value) =
            row.ok_or_else(|| ApiError::conflict("staging_managed_base_candidate_unavailable"))?;
        let candidate: ManagedBaseCandidate =
            serde_json::from_value(value).map_err(|_| invalid())?;
        candidate.validate()?;
        self.matches(&candidate, now)?;
        if candidate.artifact_id != artifact
            || candidate.architecture != arch
            || json!(candidate.capabilities) != caps
        {
            return Err(invalid());
        }
        Ok(candidate)
    }
    fn row_binding(&self, row: &Value) -> Result<(), ApiError> {
        let config = &row["request_config"];
        self.require_row_actor(row)?;
        if !self.applies(
            row["account_id"].as_str().unwrap_or(""),
            row["provider"].as_str().unwrap_or(""),
        ) || config["managedReleaseId"] != self.release_id
            || config["managedCandidateSha256"] != self.candidate_sha256
            || config["managedCandidateArtifactId"] != self.artifact_id
        {
            return Err(ApiError::forbidden("staging_managed_base_candidate_fenced"));
        }
        Ok(())
    }
    /// Reuse a caller-owned checkout; the registry lock is still committed
    /// before the caller may acquire the workspace lifecycle lock.
    pub(crate) async fn row_allowed_on(
        &self,
        connection: &mut sqlx::PgConnection,
        row: &Value,
        now: i64,
    ) -> Result<(), ApiError> {
        let effective = self.pin_for_row(row)?;
        let mut tx = sqlx::Connection::begin(connection).await?;
        effective.load(&mut tx, now, true).await?;
        self.retained_row_inventory(&mut tx, &effective, row)
            .await?;
        tx.commit().await?;
        Ok(())
    }
    pub(crate) async fn row_allowed(
        &self,
        pool: &PgPool,
        row: &Value,
        now: i64,
    ) -> Result<(), ApiError> {
        let effective = self.pin_for_row(row)?;
        let mut tx = pool.begin().await?;
        effective.load(&mut tx, now, true).await?;
        self.retained_row_inventory(&mut tx, &effective, row)
            .await?;
        tx.commit().await?;
        Ok(())
    }
}
pub(crate) async fn compute_allowance(
    tx: &mut Transaction<'_, Postgres>,
    pin: Option<&StagingBasePin>,
    row: &Value,
    now: i64,
) -> Result<bool, ApiError> {
    if !row["request_config"]["managedCandidateSha256"].is_string() {
        return Ok(false);
    }
    let pin =
        pin.ok_or_else(|| ApiError::conflict("staging_managed_base_candidate_unavailable"))?;
    let effective = pin.pin_for_row(row)?;
    let parent = pin;
    let pin = &effective;
    // Match creation's account -> base-row order; never invert these locks.
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
        .bind(format!("cloud-create:{}", pin.owner_id))
        .execute(&mut **tx)
        .await?;
    // Revalidate the exact stored artifact, manifest, harness and candidate hash,
    // including expiry and preparing-only state, before any provider effect.
    let workspace = row["workspace_id"].as_str().ok_or_else(invalid)?;
    // The advisory lock above must finish in a separate statement: this
    // statement's READ COMMITTED snapshot then sees a preceding creator's commit.
    // Registry lock, schema probe and bounded-computer inventory share that
    // snapshot and one network execution; the registry function still locks the
    // actual returned row under the restricted app role.
    let (schema, record, count, owns): (bool, Option<Value>, i64, Option<bool>) =
        sqlx::query_as("SELECT EXISTS(SELECT 1 FROM information_schema.columns WHERE table_schema=current_schema() AND table_name='cloud_managed_bases' AND column_name='candidate_manifest'),(SELECT to_jsonb(b) FROM zuse_rust_managed_base_admission_lock($1,$2) b),count(*),bool_or(workspace_id=$4) FROM api_cloud_workspaces WHERE account_id=$3 AND state<>'deleted' AND request_config ? 'managedCandidateSha256'")
            .bind(pin.provider.as_str()).bind(&pin.release_id).bind(&pin.owner_id).bind(workspace)
            .fetch_one(&mut **tx).await.map_err(super::admission_lock::error)?;
    if !schema {
        return Err(ApiError::conflict(
            "staging_managed_base_candidate_schema_required",
        ));
    }
    pin.candidate_from_locked_record(
        &record.ok_or_else(|| ApiError::conflict("staging_managed_base_candidate_unavailable"))?,
        now,
    )?;
    parent.retained_row_inventory(tx, pin, row).await?;
    if !pin.allow_staging_compute {
        return Ok(false);
    }
    if count < 1 || pin.computer_limit().is_some_and(|limit| count > limit) || owns != Some(true) {
        return Err(ApiError::conflict("staging_candidate_computer_limit"));
    }
    Ok(true)
}

pub(crate) async fn source_for(
    tx: &mut Transaction<'_, Postgres>,
    pin: Option<&StagingBasePin>,
    owner: &str,
    provider: &str,
    agent: &str,
    model: &str,
    size: &str,
    organization: bool,
    now: i64,
) -> Result<Value, ApiError> {
    if let Some(pin) = pin.filter(|p| p.applies(owner, provider)) {
        if organization != owner.starts_with("organization:")
            || organization && pin.actor_id.is_none()
        {
            return Err(ApiError::forbidden(
                "staging_candidate_actor_scope_required",
            ));
        }
        let c = pin.load(tx, now, true).await?;
        let agent = if agent == "claude-code" {
            "claude"
        } else {
            agent
        };
        if !c
            .harnesses
            .iter()
            .any(|h| h.agent.as_str() == agent && h.catalog.resolve_model(model).is_some())
            || !c.capabilities.sizes.iter().any(|s| s.id == size)
        {
            return Err(ApiError::conflict(
                "staging_candidate_unavailable_for_agent",
            ));
        }
        return Ok(json!({"build_id":null,"snapshot_id":c.artifact_id,"settings":c.settings(pin)}));
    }
    super::source_for(tx, provider, agent, model, size, organization).await
}

impl crate::store::Store {
    /// Discovery must describe an unusable pin without turning account reads into
    /// admission attempts. The strict loader remains authoritative for writes.
    pub(crate) async fn staging_base_candidate_availability(
        &self,
        owner: &str,
        now: i64,
    ) -> Result<
        (
            Option<(Arc<StagingBasePin>, ManagedBaseCandidate)>,
            Option<&'static str>,
        ),
        ApiError,
    > {
        match self.staging_base_candidate(owner, now).await {
            Ok(candidate) => Ok((candidate, None)),
            Err(error) if error.status.is_client_error() => Ok((None, Some(error.code))),
            Err(error) => Err(error),
        }
    }
    pub(crate) async fn staging_base_candidate(
        &self,
        owner: &str,
        now: i64,
    ) -> Result<Option<(Arc<StagingBasePin>, ManagedBaseCandidate)>, ApiError> {
        let Some(pin) = self
            .staging_candidate
            .as_ref()
            .filter(|p| p.owner_id == owner)
        else {
            return Ok(None);
        };
        let mut tx = self.pool.begin().await?;
        let candidate = pin.load(&mut tx, now, false).await?;
        tx.commit().await?;
        Ok(Some((pin.clone(), candidate)))
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
pub(crate) async fn fixture_registered_pin(
    pool: &PgPool,
    owner: &str,
    now: i64,
) -> Result<Arc<StagingBasePin>, ApiError> {
    let candidate = tests::candidate();
    let pin = Arc::new(candidate.pin(owner, now + 3_600_000, true)?);
    candidate.stage(pool, &pin, now).await?;
    Ok(pin)
}

#[cfg(test)]
mod pilot_capacity_tests {
    use super::*;
    #[test]
    fn two_computers_require_the_explicit_staging_compute_allowance() {
        let mut pin = StagingBasePin {
            owner_id: "owner".into(),
            actor_id: None,
            provider: ProviderId::Boxd,
            release_id: "release".into(),
            artifact_id: "artifact".into(),
            harness_release: "a".repeat(64),
            manifest_sha256: "b".repeat(64),
            candidate_sha256: "c".repeat(64),
            expires_at: 1,
            allow_staging_compute: false,
            max_computers: None,
            unlimited_computers: false,
            retained_workspaces: Vec::new(),
        };
        assert_eq!(pin.computer_limit(), Some(1));
        pin.allow_staging_compute = true;
        assert_eq!(pin.computer_limit(), Some(2));
        let mut configured = serde_json::to_value(&pin).unwrap();
        configured["maxComputers"] = json!(4);
        let configured: StagingBasePin = serde_json::from_value(configured).unwrap();
        assert!(configured.validate().is_ok());
        assert_eq!(configured.computer_limit(), Some(4));
        assert!(
            configured
                .deployment("production", &["owner".into()], &["owner".into()], 0)
                .is_err()
        );
        for limit in [0, 1, 11, u16::MAX] {
            let mut invalid = configured.clone();
            invalid.max_computers = Some(limit);
            assert!(invalid.validate().is_err());
        }
        let mut invalid = configured;
        invalid.allow_staging_compute = false;
        assert!(invalid.validate().is_err());
        let mut unlimited = pin.clone();
        unlimited.unlimited_computers = true;
        assert!(unlimited.validate().is_ok());
        assert_eq!(unlimited.computer_limit(), None);
        assert!(
            unlimited
                .deployment("production", &["owner".into()], &["owner".into()], 0)
                .is_err()
        );
        unlimited.max_computers = Some(10);
        assert!(unlimited.validate().is_err());
        unlimited.max_computers = None;
        unlimited.allow_staging_compute = false;
        assert!(unlimited.validate().is_err());
    }
}

#[cfg(test)]
mod retained_tests;

#[cfg(test)]
pub(crate) fn fixture_project_cache_candidate() -> ManagedBaseCandidate {
    let mut candidate = tests::candidate();
    candidate.release_id = "release".into();
    candidate.artifact_id = "base-artifact".into();
    candidate.runtime_sha256 = "a".repeat(64);
    candidate.initial_runtime_bootstrap = Some(super::bootstrap::InitialRuntimeBootstrap {
        version: 1,
        mode: "stopped-memory-snapshot-v1".into(),
        runtime_sha256: "a".repeat(64),
        supervisor_sha256: "b".repeat(64),
        service_sha256: "c".repeat(64),
        manifest_sha256: "d".repeat(64),
        helper_sha256: "e".repeat(64),
    });
    candidate.validate().unwrap();
    candidate
}
