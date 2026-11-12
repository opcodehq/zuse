//! Operator-published, immutable managed bases. Customer image cleanup never owns these artifacts.
mod admission_validation;
pub mod bootstrap;
use crate::error::ApiError;
pub(crate) use admission_validation::source_from_locked_records;
mod admission_lock;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::{PgPool, Postgres, Transaction};
use zuse_cloud_runtime::image::ImageManifest;
use zuse_protocol::cloud_computers::{AgentId, ProviderCapabilities, ProviderId};

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PinnedHarness {
    pub agent: AgentId,
    pub version: String,
    pub sha256: String,
    pub catalog: zuse_protocol::cloud_catalog::CloudAgentModels,
}

pub type CertifiedHarness = PinnedHarness;

pub mod candidate;

/// The certification report is an operator-owned release artifact, not a user's image.
/// Registration records a completed certification; it does not run or invent its tests.
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ManagedBaseRelease {
    pub provider: ProviderId,
    pub release_id: String,
    pub artifact_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_artifact_binding: Option<crate::providers::artifact::ArtifactBinding>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initial_runtime_bootstrap: Option<bootstrap::InitialRuntimeBootstrap>,
    pub architecture: String,
    pub capabilities: ProviderCapabilities,
    pub manifest: ImageManifest,
    pub harnesses: Vec<CertifiedHarness>,
    pub certification_report_sha256: String,
}

fn validate_provider_binding(
    provider: ProviderId,
    artifact: &str,
    binding: Option<&crate::providers::artifact::ArtifactBinding>,
) -> Result<(), ApiError> {
    if provider == ProviderId::E2b && binding.is_none() {
        return Err(ApiError::bad_request(
            "managed_base_provider_version_required",
        ));
    }
    if let Some(binding) = binding {
        binding
            .validate(provider, artifact)
            .map_err(|_| ApiError::bad_request("managed_base_provider_binding_invalid"))?;
    }
    Ok(())
}
fn identifier(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 256
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._/-:".contains(&b))
}
fn sha256(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

fn validate_content(
    provider: ProviderId,
    release_id: &str,
    artifact_id: &str,
    architecture: &str,
    c: &ProviderCapabilities,
    m: &ImageManifest,
    harnesses: &[PinnedHarness],
) -> Result<(), ApiError> {
    if !identifier(&release_id)
        || !identifier(&artifact_id)
        || !matches!(architecture, "x86_64" | "aarch64")
        || c.id != provider
        || !c.can_create()
        || c.command_admission_version.is_some_and(|version| {
            version != zuse_protocol::cloud_task_commands::VERSION
                || m.rust_command_admission_version != Some(version)
        })
        || c.unavailable_reason.is_some()
        || c.sizes
            .iter()
            .any(|s| !identifier(&s.id) || s.vcpus == 0 || s.memory_mib == 0 || s.disk_gib == 0)
        || !m.managed_base
        || !m.repositories.is_empty()
        || !m.credentials_removed
        || m.rust_memory_sleep_version.is_some_and(|v| v != 1)
        || m.rust_tool_output_version.is_some_and(|v| v != 1)
        || m.rust_github_credentials_version.is_some_and(|v| v != 1)
        || m.rust_github_cli_version.is_some_and(|v| v != 1)
        || (m.rust_github_cli_version == Some(1)) != m.github_cli.is_some()
        || m.github_cli.as_ref().is_some_and(|cli| {
            !cli.version.starts_with("gh version 2.")
                || cli.version.len() > 1024
                || cli.version.chars().any(char::is_control)
                || !sha256(&cli.sha256)
                || cli.wrapper_sha256.as_ref().is_some_and(|sha| !sha256(sha))
                || (cli.wrapper_sha256.is_some() != cli.login_shell_resolved_managed.is_some())
                || cli.login_shell_resolved_managed == Some(false)
        })
        || m.rust_command_admission_version
            .is_some_and(|v| v != zuse_protocol::cloud_task_commands::VERSION)
        || (m.rust_command_admission_version == Some(zuse_protocol::cloud_task_commands::VERSION))
            != m.command_admission.is_some()
        || m.command_admission.as_ref().is_some_and(|receipt| {
            !receipt.valid()
                || m.rust_runtime_supervisor_version != Some(1)
                || m.github_cli.as_ref().is_none_or(|cli| {
                    cli.wrapper_sha256.as_ref().is_none_or(|v| !sha256(v))
                        || cli.login_shell_resolved_managed != Some(true)
                })
        })
        || m.rust_organization_execution_version
            .is_some_and(|v| v != 1)
        || m.rust_runtime_supervisor_version.is_some_and(|v| v != 1)
        || m.rust_runtime_repair_version.is_some_and(|v| v != 1)
        || m.rust_recreation_version.is_some_and(|v| v != 1)
        || m.rust_workload_secrets_version.is_some_and(|v| v != 1)
        || m.version != 1
        || m.build_id != release_id
        || !sha256(&m.plan_digest)
        || m.runtime_rpc_protocol != zuse_protocol::cloud_gateway::RUST_RPC
        || m.rust_runtime_storage_format != zuse_protocol::cloud::RUST_RUNTIME_STORAGE_FORMAT
        || m.rust_session_command_version != 1
        || m.rust_environment_recipe_version != Some(1)
        || m.rust_repository_fetch_version != Some(1)
        || m.rust_launch_model_config_version != Some(1)
        || m.rust_launch_assets_version != Some(1)
        || harnesses.is_empty()
    {
        return Err(ApiError::bad_request("managed_base_release_invalid"));
    }
    let mut agents = std::collections::HashSet::new();
    for harness in harnesses {
        let expected = match harness.agent {
            AgentId::Claude => zuse_protocol::HarnessId::ClaudeCode,
            AgentId::Codex => zuse_protocol::HarnessId::Codex,
            AgentId::Grok => zuse_protocol::HarnessId::Grok,
        };
        let mut model_ids = std::collections::HashSet::new();
        if !agents.insert(harness.agent.as_str())
            || !identifier(&harness.version)
            || !sha256(&harness.sha256)
            || harness.catalog.id != expected
            || harness.catalog.models.is_empty()
            || harness.catalog.models.len() > 256
            || harness
                .catalog
                .resolve_model(&harness.catalog.default_model)
                .is_none()
            || harness
                .catalog
                .models
                .iter()
                .any(|m| !identifier(&m.id) || !model_ids.insert(&m.id))
        {
            return Err(ApiError::bad_request("managed_base_harness_invalid"));
        }
    }
    let mut sizes = std::collections::HashSet::new();
    if c.sizes.iter().any(|s| !sizes.insert(&s.id)) {
        return Err(ApiError::bad_request("managed_base_sizes_invalid"));
    }
    Ok(())
}

impl crate::store::Store {
    /// Only promoted harnesses with a usable account are launch choices. Legacy deployments
    /// retain their existing catalogue until they opt into the credential authority.
    pub(crate) async fn managed_launch_catalog(
        &self,
        connections: &[zuse_protocol::cloud_connections::AgentConnection],
    ) -> Result<zuse_protocol::cloud_catalog::CloudAgentCatalog, ApiError> {
        self.managed_launch_catalog_for("", connections).await
    }
    pub(crate) async fn managed_launch_catalog_for(
        &self,
        owner: &str,
        connections: &[zuse_protocol::cloud_connections::AgentConnection],
    ) -> Result<zuse_protocol::cloud_catalog::CloudAgentCatalog, ApiError> {
        let catalogues = self
            .provider_launch_catalogs_for(owner, connections)
            .await?;
        let mut seen = std::collections::HashSet::new();
        let mut providers = Vec::new();
        for catalog in catalogues.into_values() {
            for agent in catalog.providers {
                if seen.insert(agent.id.clone()) {
                    providers.push(agent);
                }
            }
        }
        Ok(zuse_protocol::cloud_catalog::CloudAgentCatalog { providers })
    }
    pub(crate) async fn provider_launch_catalogs_for(
        &self,
        owner: &str,
        connections: &[zuse_protocol::cloud_connections::AgentConnection],
    ) -> Result<
        std::collections::BTreeMap<String, zuse_protocol::cloud_catalog::CloudAgentCatalog>,
        ApiError,
    > {
        let mut catalogs = std::collections::BTreeMap::new();
        for id in ProviderId::ALL {
            catalogs.insert(
                id.as_str().to_owned(),
                zuse_protocol::cloud_catalog::CloudAgentCatalog { providers: vec![] },
            );
        }
        let schema:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM information_schema.columns WHERE table_schema=current_schema() AND table_name='cloud_managed_bases' AND column_name='runtime_manifest')").fetch_one(&self.pool).await?;
        if !schema {
            return Ok(catalogs);
        }
        let records:Vec<(String,Value)>=sqlx::query_as("SELECT provider,runtime_manifest FROM cloud_managed_bases WHERE state='verified' AND runtime_manifest IS NOT NULL ORDER BY created_at DESC,release_id DESC").fetch_all(&self.pool).await?;
        let mut seen = std::collections::HashSet::new();
        for (provider, record) in records {
            let release: ManagedBaseRelease = serde_json::from_value(record)
                .map_err(|_| ApiError::unavailable("managed_base_manifest_invalid"))?;
            release
                .validate()
                .map_err(|_| ApiError::unavailable("managed_base_manifest_invalid"))?;
            if release.provider.as_str() != provider {
                return Err(ApiError::unavailable("managed_base_manifest_invalid"));
            }
            if owner.starts_with("organization:")
                && release.manifest.rust_organization_execution_version != Some(1)
            {
                continue;
            }
            if release.provider == ProviderId::Boxd
                && release.manifest.rust_runtime_supervisor_version != Some(1)
            {
                continue;
            }
            let Some(catalog) = catalogs.get_mut(&provider) else {
                continue;
            };
            for harness in release.harnesses {
                if connections.iter().any(|c| {
                    c.agent == harness.agent
                        && c.state == zuse_protocol::cloud_connections::ConnectionState::Connected
                }) && seen.insert((provider.clone(), harness.agent.as_str()))
                {
                    catalog.providers.push(harness.catalog);
                }
            }
        }
        let (candidate, unavailable) = self
            .staging_base_candidate_availability(owner, chrono::Utc::now().timestamp_millis())
            .await?;
        if unavailable.is_some() {
            // An owner pin is an exact selection, never permission to fall back
            // to a different promoted artifact when that selection is unusable.
            if let Some(pin) = self
                .staging_candidate
                .as_ref()
                .filter(|p| p.owner_id == owner)
            {
                catalogs.insert(
                    pin.provider.as_str().into(),
                    zuse_protocol::cloud_catalog::CloudAgentCatalog { providers: vec![] },
                );
            }
        }
        if let Some((pin, candidate)) = candidate {
            let providers = candidate
                .harnesses
                .into_iter()
                .filter(|h| {
                    connections.iter().any(|c| {
                        c.agent == h.agent
                            && c.state
                                == zuse_protocol::cloud_connections::ConnectionState::Connected
                    })
                })
                .map(|h| h.catalog)
                .collect();
            catalogs.insert(
                pin.provider.as_str().into(),
                zuse_protocol::cloud_catalog::CloudAgentCatalog { providers },
            );
        }
        Ok(catalogs)
    }
}
impl ManagedBaseRelease {
    pub async fn register_url(&self, url: &str, now: i64) -> Result<(), ApiError> {
        self.validate()?;
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .connect(url)
            .await
            .map_err(|_| ApiError::unavailable("managed_base_database_unavailable"))?;
        self.register(&pool, now).await
    }
    pub fn validate(&self) -> Result<(), ApiError> {
        if !sha256(&self.certification_report_sha256) {
            return Err(ApiError::bad_request("managed_base_release_invalid"));
        }
        self.validate_provider_binding()?;
        validate_content(
            self.provider,
            &self.release_id,
            &self.artifact_id,
            &self.architecture,
            &self.capabilities,
            &self.manifest,
            &self.harnesses,
        )
    }

    fn validate_provider_binding(&self) -> Result<(), ApiError> {
        if let Some(spec) = &self.initial_runtime_bootstrap {
            spec.validate(self.provider, &self.manifest)?;
        }
        validate_provider_binding(
            self.provider,
            &self.artifact_id,
            self.provider_artifact_binding.as_ref(),
        )
    }
    pub async fn register(&self, pool: &PgPool, now: i64) -> Result<(), ApiError> {
        self.validate()?;
        let value = serde_json::to_value(self)
            .map_err(|_| ApiError::bad_request("managed_base_release_invalid"))?;
        let mut tx = pool.begin().await?;
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
            .bind(format!(
                "managed-base:{}:{}",
                self.provider.as_str(),
                self.release_id
            ))
            .execute(&mut *tx)
            .await?;
        let prior: Option<(Option<Value>, String)> = sqlx::query_as("SELECT runtime_manifest,state FROM cloud_managed_bases WHERE provider=$1 AND release_id=$2 FOR UPDATE")
            .bind(self.provider.as_str()).bind(&self.release_id).fetch_optional(&mut *tx).await?;
        if let Some((manifest, state)) = prior {
            if manifest.as_ref() != Some(&value) || state != "verified" {
                return Err(ApiError::conflict("managed_base_release_immutable"));
            }
        } else {
            sqlx::query("INSERT INTO cloud_managed_bases(provider,release_id,artifact_id,architecture,capabilities,state,created_at,runtime_manifest) VALUES($1,$2,$3,$4,$5,'verified',$6,$7)")
                .bind(self.provider.as_str()).bind(&self.release_id).bind(&self.artifact_id).bind(&self.architecture)
                .bind(json!(self.capabilities)).bind(now).bind(value).execute(&mut *tx).await?;
        }
        tx.commit().await?;
        Ok(())
    }

    fn settings(&self) -> Value {
        let mut value = manifest_settings(
            self.provider,
            &self.release_id,
            &self.capabilities,
            &self.manifest,
        );
        if let Some(binding) = &self.provider_artifact_binding {
            value["providerArtifactBinding"] = json!(binding);
        }
        if let Some(spec) = &self.initial_runtime_bootstrap {
            value["initialRuntimeBootstrap"] = json!(spec);
        }
        value
    }
}

fn manifest_settings(
    provider: ProviderId,
    release_id: &str,
    capabilities: &ProviderCapabilities,
    manifest: &ImageManifest,
) -> Value {
    let mut settings = json!({"runtimeRpcProtocol":manifest.runtime_rpc_protocol,
            "rustRuntimeStorageFormat":manifest.rust_runtime_storage_format,
            "rustSessionCommandVersion":1,"providerAuthDeliveryVersion":1,
            "codexAuthDeliveryVersion":1,"grokAuthDeliveryVersion":1,
            "rustLaunchAssetsVersion":1,"rustLaunchModelConfigVersion":1,
            "rustEnvironmentRecipeVersion":1,"rustRepositoryFetchVersion":1,
            "managedReleaseId":release_id});
    if manifest.rust_tool_output_version == Some(1) {
        settings["rustToolOutputVersion"] = json!(1);
    }
    if manifest.rust_github_cli_version == Some(1)
        && manifest.github_cli.as_ref().is_some_and(|cli| {
            cli.wrapper_sha256.as_ref().is_some_and(|sha| sha256(sha))
                && cli.login_shell_resolved_managed == Some(true)
        })
    {
        settings["rustGithubCliVersion"] = json!(1);
    }
    if manifest.rust_command_admission_version == Some(zuse_protocol::cloud_task_commands::VERSION)
        && manifest
            .command_admission
            .as_ref()
            .is_some_and(|receipt| receipt.valid())
    {
        settings["rustCommandAdmissionVersion"] =
            json!(zuse_protocol::cloud_task_commands::VERSION);
    }
    if manifest.rust_github_credentials_version == Some(1) {
        settings["rustGithubCredentialsVersion"] = json!(1);
    }
    if manifest.rust_organization_execution_version == Some(1) {
        settings["rustOrganizationExecutionVersion"] = json!(1);
    }
    if manifest.rust_runtime_supervisor_version == Some(1) {
        settings["rustRuntimeSupervisorVersion"] = json!(1);
    }
    if manifest.rust_workload_secrets_version == Some(1) {
        settings["rustWorkloadSecretsVersion"] = json!(1);
    }
    if manifest.rust_recreation_version == Some(1) {
        settings["rustRecreationVersion"] = json!(1);
    }
    if manifest.rust_runtime_repair_version == Some(1) {
        settings["rustRuntimeRepairVersion"] = json!(1);
    }
    if manifest.rust_memory_sleep_version == Some(1)
        && capabilities.preserves_memory
        && matches!(provider, ProviderId::Boxd | ProviderId::E2b)
    {
        settings["rustMemorySleepVersion"] = json!(1);
    }
    settings
}

/// Read lock pins the immutable release through allocation admission, including retirement races.
pub(crate) async fn source_for(
    tx: &mut Transaction<'_, Postgres>,
    provider: &str,
    agent: &str,
    model: &str,
    size: &str,
    organization: bool,
) -> Result<Value, ApiError> {
    let ready: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM information_schema.columns WHERE table_schema=current_schema() AND table_name='cloud_managed_bases' AND column_name='runtime_manifest')")
        .fetch_one(&mut **tx).await?;
    if !ready {
        return Err(ApiError::conflict("managed_base_unavailable"));
    }
    let records:Vec<String>=sqlx::query_scalar("SELECT release_id FROM cloud_managed_bases WHERE provider=$1 AND state='verified' AND runtime_manifest IS NOT NULL ORDER BY created_at DESC,release_id DESC").bind(provider).fetch_all(&mut **tx).await?;
    let agent = if agent == "claude-code" {
        "claude"
    } else {
        agent
    };
    let mut supervisor_missing = false;
    let mut organization_missing = false;
    for id in records {
        let Some(row) = admission_lock::record(tx, provider, &id).await? else {
            continue;
        };
        if row["state"] != "verified" || row["runtime_manifest"].is_null() {
            continue;
        }
        let artifact = &row["artifact_id"];
        let architecture = &row["architecture"];
        let capabilities = &row["capabilities"];
        let release: ManagedBaseRelease =
            serde_json::from_value(row["runtime_manifest"].clone())
                .map_err(|_| ApiError::unavailable("managed_base_manifest_invalid"))?;
        release
            .validate()
            .map_err(|_| ApiError::unavailable("managed_base_manifest_invalid"))?;
        if release.provider.as_str() != provider
            || release.release_id != id
            || json!(release.artifact_id) != *artifact
            || json!(release.architecture) != *architecture
            || json!(release.capabilities) != *capabilities
        {
            return Err(ApiError::unavailable("managed_base_manifest_invalid"));
        }
        let eligible = release
            .harnesses
            .iter()
            .any(|h| h.agent.as_str() == agent && h.catalog.resolve_model(model).is_some())
            && release.capabilities.sizes.iter().any(|s| s.id == size);
        if eligible {
            if release.provider == ProviderId::Boxd
                && release.manifest.rust_runtime_supervisor_version != Some(1)
            {
                supervisor_missing = true;
                continue;
            }
            if organization && release.manifest.rust_organization_execution_version != Some(1) {
                organization_missing = true;
                continue;
            }
            return Ok(
                json!({"build_id":null,"snapshot_id":release.artifact_id,"settings":release.settings()}),
            );
        }
    }
    Err(ApiError::conflict(if supervisor_missing {
        "managed_base_runtime_supervisor_required"
    } else if organization_missing {
        "managed_base_organization_runtime_required"
    } else {
        "managed_base_unavailable_for_agent"
    }))
}

/// Retirement stops new admission. Existing archives retain their exact compatible base;
/// a missing or failed release requires operator recovery rather than a silent upgrade.
pub(crate) async fn retained_source(
    tx: &mut Transaction<'_, Postgres>,
    provider: &str,
    release_id: &str,
) -> Result<String, ApiError> {
    let row = admission_lock::record(tx, provider, release_id)
        .await?
        .filter(|row| {
            matches!(row["state"].as_str(), Some("verified" | "retired"))
                && !row["runtime_manifest"].is_null()
        })
        .ok_or_else(|| ApiError::conflict("cloud_restore_managed_base_required"))?;
    let artifact = row["artifact_id"]
        .as_str()
        .ok_or_else(|| ApiError::unavailable("managed_base_manifest_invalid"))?;
    let release: ManagedBaseRelease = serde_json::from_value(row["runtime_manifest"].clone())
        .map_err(|_| ApiError::unavailable("managed_base_manifest_invalid"))?;
    release
        .validate()
        .map_err(|_| ApiError::unavailable("managed_base_manifest_invalid"))?;
    if release.provider.as_str() != provider
        || release.release_id != release_id
        || release.artifact_id != artifact
    {
        return Err(ApiError::unavailable("managed_base_manifest_invalid"));
    }
    Ok(artifact.to_owned())
}

/// A newer base an explicit "Update computer" recreation may move onto.
pub(crate) struct RecreationTarget {
    pub release_id: String,
    pub artifact_id: String,
    /// `ManagedBaseRelease::settings()`: the release-derived workspace config.
    pub settings: Value,
}

/// What a computer needs from a base to restore its captured state there.
pub(crate) struct RecreationNeeds<'a> {
    pub agent: &'a str,
    pub size: &'a str,
    pub organization: bool,
}

/// The restore-relevant identity of a base, verified release or staging
/// candidate alike.
pub(crate) struct RecreationBase<'a> {
    pub provider: ProviderId,
    pub architecture: &'a str,
    pub capabilities: &'a ProviderCapabilities,
    pub manifest: &'a ImageManifest,
    pub harnesses: &'a [PinnedHarness],
}
impl<'a> From<&'a ManagedBaseRelease> for RecreationBase<'a> {
    fn from(r: &'a ManagedBaseRelease) -> Self {
        Self {
            provider: r.provider,
            architecture: &r.architecture,
            capabilities: &r.capabilities,
            manifest: &r.manifest,
            harnesses: &r.harnesses,
        }
    }
}

/// The archive written by the current base must be restorable on the target:
/// same provider and architecture, the same wire protocol and storage format,
/// and explicit recreation (verified archive restore) support. The computer's
/// agent and size must still be offered, under the same supervisor and
/// organization rules as new computers.
pub(crate) fn recreation_compatible(
    current: RecreationBase<'_>,
    candidate: RecreationBase<'_>,
    needs: &RecreationNeeds<'_>,
) -> bool {
    let agent = if needs.agent == "claude-code" {
        "claude"
    } else {
        needs.agent
    };
    candidate.provider == current.provider
        && candidate.architecture == current.architecture
        && candidate.manifest.runtime_rpc_protocol == current.manifest.runtime_rpc_protocol
        && candidate.manifest.rust_runtime_storage_format
            == current.manifest.rust_runtime_storage_format
        && candidate.manifest.rust_recreation_version == Some(1)
        && candidate
            .harnesses
            .iter()
            .any(|h| h.agent.as_str() == agent)
        && candidate
            .capabilities
            .sizes
            .iter()
            .any(|s| s.id == needs.size)
        && (candidate.provider != ProviderId::Boxd
            || candidate.manifest.rust_runtime_supervisor_version == Some(1))
        && (!needs.organization
            || candidate.manifest.rust_organization_execution_version == Some(1))
        && feature_versions(current.manifest)
            .into_iter()
            .zip(feature_versions(candidate.manifest))
            .all(|(had, has)| had.is_none() || has.is_some_and(|has| Some(has) >= had))
}

/// Opt-in base features recorded on a computer. Moving to a base without one
/// would leave its stored config claiming a feature the guest lacks.
fn feature_versions(m: &ImageManifest) -> [Option<u32>; 14] {
    [
        m.rust_memory_sleep_version,
        m.rust_tool_output_version,
        m.rust_workload_secrets_version,
        m.rust_repository_fetch_version,
        m.rust_github_credentials_version,
        m.rust_github_cli_version,
        m.rust_organization_execution_version,
        m.rust_runtime_supervisor_version,
        m.rust_runtime_repair_version,
        m.rust_command_admission_version,
        m.rust_environment_recipe_version,
        m.rust_slack_version,
        m.rust_launch_model_config_version,
        m.rust_launch_assets_version,
    ]
}

async fn locked_release(
    tx: &mut Transaction<'_, Postgres>,
    provider: &str,
    release_id: &str,
    states: &[&str],
) -> Result<Option<ManagedBaseRelease>, ApiError> {
    let Some(row) = admission_lock::record(tx, provider, release_id).await? else {
        return Ok(None);
    };
    if !row["state"].as_str().is_some_and(|s| states.contains(&s))
        || row["runtime_manifest"].is_null()
    {
        return Ok(None);
    }
    let release: ManagedBaseRelease = serde_json::from_value(row["runtime_manifest"].clone())
        .map_err(|_| ApiError::unavailable("managed_base_manifest_invalid"))?;
    release
        .validate()
        .map_err(|_| ApiError::unavailable("managed_base_manifest_invalid"))?;
    if release.provider.as_str() != provider
        || release.release_id != release_id
        || json!(release.artifact_id) != row["artifact_id"]
    {
        return Err(ApiError::unavailable("managed_base_manifest_invalid"));
    }
    Ok(Some(release))
}

/// Newest verified base this computer can be recreated onto. Returns the
/// current release when nothing newer is compatible (an ordinary recreation).
pub(crate) async fn latest_recreation_target(
    tx: &mut Transaction<'_, Postgres>,
    provider: &str,
    current_release_id: &str,
    needs: &RecreationNeeds<'_>,
) -> Result<RecreationTarget, ApiError> {
    let current = locked_release(tx, provider, current_release_id, &["verified", "retired"])
        .await?
        .ok_or_else(|| ApiError::conflict("cloud_restore_managed_base_required"))?;
    // Strictly newer than the pinned release: never a silent downgrade, even
    // when the pinned release has since been retired.
    let records:Vec<String>=sqlx::query_scalar("SELECT b.release_id FROM cloud_managed_bases b JOIN cloud_managed_bases c ON c.provider=b.provider AND c.release_id=$2 WHERE b.provider=$1 AND b.state='verified' AND b.runtime_manifest IS NOT NULL AND (b.created_at,b.release_id)>(c.created_at,c.release_id) ORDER BY b.created_at DESC,b.release_id DESC").bind(provider).bind(current_release_id).fetch_all(&mut **tx).await?;
    for id in records {
        let Some(candidate) = locked_release(tx, provider, &id, &["verified"]).await? else {
            continue;
        };
        if recreation_compatible((&current).into(), (&candidate).into(), needs) {
            return Ok(RecreationTarget {
                release_id: candidate.release_id.clone(),
                artifact_id: candidate.artifact_id.clone(),
                settings: candidate.settings(),
            });
        }
    }
    Ok(RecreationTarget {
        release_id: current.release_id.clone(),
        artifact_id: current.artifact_id.clone(),
        settings: current.settings(),
    })
}

/// The release a recreation already chose. Like `retained_source`, a target
/// retired after admission still restores; a missing one needs an operator.
pub(crate) async fn recreation_target(
    tx: &mut Transaction<'_, Postgres>,
    provider: &str,
    release_id: &str,
) -> Result<RecreationTarget, ApiError> {
    let release = locked_release(tx, provider, release_id, &["verified", "retired"])
        .await?
        .ok_or_else(|| ApiError::conflict("cloud_restore_managed_base_required"))?;
    Ok(RecreationTarget {
        release_id: release.release_id.clone(),
        artifact_id: release.artifact_id.clone(),
        settings: release.settings(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    pub(super) fn release() -> ManagedBaseRelease {
        serde_json::from_value(json!({
            "provider":"boxd","releaseId":"managed-fixture-v1","artifactId":"platform-snapshot","architecture":"x86_64",
            "certificationReportSha256":"a".repeat(64),
            "capabilities":{"id":"boxd","configured":true,"managedBaseReady":true,"preservesDisk":true,"preservesMemory":true,"supportsSnapshots":true,"supportsDocker":true,"unavailableReason":null,"sizes":[{"id":"default","label":"Standard","vcpus":2,"memoryMib":2048,"diskGib":20}]},
            "harnesses":[{"agent":"codex","version":"fixture-1","sha256":"b".repeat(64),"catalog":{"id":"codex","name":"Codex","defaultModel":"fixture","models":[{"id":"fixture","label":"Fixture"}]}}],
            "manifest":{"managedBase":true,"rustRuntimeSupervisorVersion":1,"version":1,"buildId":"managed-fixture-v1","templateVersion":"fixture","configurationDigest":"fixture","planDigest":"c".repeat(64),"runtimeRpcProtocol":"zuse-rpc-v1","rustRuntimeStorageFormat":zuse_protocol::cloud::RUST_RUNTIME_STORAGE_FORMAT,"rustSessionCommandVersion":1,"credentialsRemoved":true,"repositories":[],"rustRepositoryFetchVersion":1,"rustEnvironmentRecipeVersion":1,"rustLaunchModelConfigVersion":1,"rustLaunchAssetsVersion":1}
        })).unwrap()
    }
    #[test]
    fn recreation_target_must_restore_the_captured_state() {
        let current = release();
        let mut newer = release();
        newer.release_id = "managed-fixture-v2".into();
        newer.manifest.rust_recreation_version = Some(1);
        let needs = RecreationNeeds {
            agent: "codex",
            size: "default",
            organization: false,
        };
        assert!(recreation_compatible(
            (&current).into(),
            (&newer).into(),
            &needs
        ));
        let rejected = |change: fn(&mut ManagedBaseRelease)| {
            let mut other = newer.clone();
            change(&mut other);
            !recreation_compatible((&current).into(), (&other).into(), &needs)
        };
        assert!(rejected(|r| r.architecture = "aarch64".into()));
        assert!(rejected(|r| r.provider = ProviderId::E2b));
        assert!(rejected(
            |r| r.manifest.rust_runtime_storage_format = "zuse-rust-engine-v2".into()
        ));
        assert!(rejected(
            |r| r.manifest.runtime_rpc_protocol = "zuse-rpc-v2".into()
        ));
        assert!(rejected(|r| r.manifest.rust_recreation_version = None));
        assert!(rejected(|r| r.harnesses.clear()));
        assert!(rejected(|r| r.capabilities.sizes.clear()));
        assert!(rejected(
            |r| r.manifest.rust_runtime_supervisor_version = None
        ));
        // A feature the computer already relies on cannot disappear.
        assert!(rejected(|r| r.manifest.rust_launch_assets_version = None));
        assert!(!recreation_compatible(
            (&current).into(),
            (&newer).into(),
            &RecreationNeeds {
                organization: true,
                ..needs
            }
        ));
    }
    #[test]
    fn github_cli_capability_requires_a_versioned_installed_binary_receipt() {
        let mut r = release();
        assert!(r.settings().get("rustGithubCliVersion").is_none());
        r.manifest.rust_github_cli_version = Some(1);
        assert!(r.validate().is_err());
        r.manifest.github_cli = Some(zuse_cloud_runtime::image::GithubCliReceipt {
            version: "gh version 2.45.0".into(),
            sha256: "a".repeat(64),
            wrapper_sha256: None,
            login_shell_resolved_managed: None,
        });
        r.validate().unwrap();
        assert!(
            r.settings().get("rustGithubCliVersion").is_none(),
            "old archives remain readable without claiming a login shim"
        );
        r.manifest.github_cli.as_mut().unwrap().wrapper_sha256 = Some("b".repeat(64));
        r.manifest
            .github_cli
            .as_mut()
            .unwrap()
            .login_shell_resolved_managed = Some(true);
        r.validate().unwrap();
        assert_eq!(r.settings()["rustGithubCliVersion"], 1);
        r.manifest.github_cli.as_mut().unwrap().sha256 = "invalid".into();
        assert!(r.validate().is_err());
        r.manifest.github_cli.as_mut().unwrap().sha256 = "a".repeat(64);
        r.manifest.rust_github_cli_version = Some(2);
        assert!(r.validate().is_err());
    }
    pub(super) fn enable_command_admission(r: &mut ManagedBaseRelease) {
        r.manifest.rust_command_admission_version =
            Some(zuse_protocol::cloud_task_commands::VERSION);
        r.manifest.command_admission = Some(zuse_cloud_runtime::image::CommandAdmissionReceipt {
            version: 1,
            runtime_sha256: "e".repeat(64),
            command_admission_version: 2,
            runtime_rpc_protocol: zuse_protocol::cloud_gateway::RUST_RPC.into(),
            storage_format: zuse_protocol::cloud::RUST_RUNTIME_STORAGE_FORMAT.into(),
            session_command_version: 1,
            execution_progress_version: 1,
            managed_agent_version: None,
        });
        r.manifest.rust_github_cli_version = Some(1);
        r.manifest.github_cli = Some(zuse_cloud_runtime::image::GithubCliReceipt {
            version: "gh version 2.79.0".into(),
            sha256: "a".repeat(64),
            wrapper_sha256: Some("b".repeat(64)),
            login_shell_resolved_managed: Some(true),
        });
    }
    #[test]
    fn command_admission_requires_exact_guest_receipt_and_managed_cli() {
        let mut release = release();
        assert!(
            release
                .settings()
                .get("rustCommandAdmissionVersion")
                .is_none()
        );
        release.manifest.rust_command_admission_version = Some(2);
        assert!(release.validate().is_err());
        enable_command_admission(&mut release);
        release.validate().unwrap();
        assert_eq!(release.settings()["rustCommandAdmissionVersion"], 2);
        release
            .manifest
            .command_admission
            .as_mut()
            .unwrap()
            .execution_progress_version = 0;
        assert!(release.validate().is_err());
        release
            .manifest
            .command_admission
            .as_mut()
            .unwrap()
            .execution_progress_version = 1;
        release.manifest.github_cli.as_mut().unwrap().wrapper_sha256 = None;
        assert!(release.validate().is_err());
    }
    #[test]
    fn memory_sleep_is_an_explicit_compatible_release_capability() {
        let mut r = release();
        assert!(r.settings().get("rustMemorySleepVersion").is_none());
        assert!(
            serde_json::to_value(&r.manifest)
                .unwrap()
                .get("rustMemorySleepVersion")
                .is_none()
        );
        r.manifest.rust_memory_sleep_version = Some(1);
        assert_eq!(r.settings()["rustMemorySleepVersion"], 1);
        r.capabilities.preserves_memory = false;
        assert!(r.settings().get("rustMemorySleepVersion").is_none());
        r.capabilities.preserves_memory = true;
        r.provider = ProviderId::Boat;
        assert!(r.settings().get("rustMemorySleepVersion").is_none());
        r = release();
        r.manifest.rust_memory_sleep_version = Some(2);
        assert!(r.validate().is_err());
    }

    #[test]
    fn optional_runtime_capabilities_never_upgrade_legacy_bases() {
        let mut r = release();
        assert!(r.settings().get("rustToolOutputVersion").is_none());
        assert!(r.settings().get("rustGithubCredentialsVersion").is_none());
        r.manifest.rust_tool_output_version = Some(1);
        r.manifest.rust_github_credentials_version = Some(1);
        assert_eq!(r.settings()["rustToolOutputVersion"], 1);
        assert_eq!(r.settings()["rustGithubCredentialsVersion"], 1);
        r.manifest.rust_tool_output_version = Some(2);
        assert!(r.validate().is_err());
        r.manifest.rust_tool_output_version = Some(1);
        r.manifest.rust_github_credentials_version = Some(2);
        assert!(r.validate().is_err());
    }

    #[tokio::test]
    #[ignore = "requires explicit isolated localhost PostgreSQL"]
    async fn all_provider_catalogs_require_certified_release_and_selected_connection() {
        use zuse_protocol::cloud_connections::{
            AgentConnection, ConnectionMethod, ConnectionState,
        };
        let url = std::env::var("ZUSE_CLOUD_TEST_DATABASE_URL").unwrap();
        assert!(matches!(
            reqwest::Url::parse(&url).unwrap().host_str(),
            Some("localhost" | "127.0.0.1" | "::1")
        ));
        let admin = PgPool::connect(&url).await.unwrap();
        let schema = format!("catalog_{}", uuid::Uuid::new_v4().simple());
        sqlx::query(&format!("CREATE SCHEMA {schema}"))
            .execute(&admin)
            .await
            .unwrap();
        let scope = schema.clone();
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .after_connect(move |c, _| {
                let scope = scope.clone();
                Box::pin(async move {
                    sqlx::query(&format!("SET search_path={scope}"))
                        .execute(c)
                        .await?;
                    Ok(())
                })
            })
            .connect(&url)
            .await
            .unwrap();
        sqlx::raw_sql("CREATE TABLE cloud_managed_bases(provider text,release_id text,state text,runtime_manifest jsonb,created_at bigint)").execute(&pool).await.unwrap();
        let store = crate::store::Store::from_pool(pool.clone());
        let mut connection = AgentConnection {
            connection_id: "selected".into(),
            agent: AgentId::Codex,
            method: ConnectionMethod::DeviceCode,
            state: ConnectionState::Connected,
            account_label: None,
            expires_at: None,
            version: 1,
            error_code: None,
            credential_update: None,
            credential_update_key: None,
        };
        assert!(
            store
                .managed_launch_catalog(&[connection.clone()])
                .await
                .unwrap()
                .providers
                .is_empty()
        );
        for id in ProviderId::ALL {
            let mut candidate = release();
            candidate.provider = id;
            candidate.capabilities.id = id;
            // E2B releases must pin their provider build.
            if id == ProviderId::E2b {
                candidate.provider_artifact_binding =
                    Some(crate::providers::artifact::ArtifactBinding {
                        version: 1,
                        mode: "private-single-build".into(),
                        artifact_id: candidate.artifact_id.clone(),
                        artifact_version: "build-one".into(),
                        artifact_name: "zuse-e2b-stage-one".into(),
                        runtime_sha256: "a".repeat(64),
                        manifest_sha256: "b".repeat(64),
                    });
            }
            sqlx::query(
                "INSERT INTO cloud_managed_bases VALUES($1,'catalog-release','pending',$2,1)",
            )
            .bind(id.as_str())
            .bind(serde_json::to_value(&candidate).unwrap())
            .execute(&pool)
            .await
            .unwrap();
            assert!(
                store
                    .managed_launch_catalog(&[connection.clone()])
                    .await
                    .unwrap()
                    .providers
                    .is_empty()
            );
            sqlx::query("UPDATE cloud_managed_bases SET state='verified'")
                .execute(&pool)
                .await
                .unwrap();
            assert_eq!(
                store
                    .managed_launch_catalog(&[connection.clone()])
                    .await
                    .unwrap()
                    .providers
                    .len(),
                1,
                "{id:?}"
            );
            connection.state = ConnectionState::Revoked;
            assert!(
                store
                    .managed_launch_catalog(&[connection.clone()])
                    .await
                    .unwrap()
                    .providers
                    .is_empty()
            );
            connection.state = ConnectionState::Connected;
            sqlx::query("UPDATE cloud_managed_bases SET state='retired'")
                .execute(&pool)
                .await
                .unwrap();
            assert!(
                store
                    .managed_launch_catalog(&[connection.clone()])
                    .await
                    .unwrap()
                    .providers
                    .is_empty()
            );
            sqlx::query("DELETE FROM cloud_managed_bases")
                .execute(&pool)
                .await
                .unwrap();
        }
        let mut bad = release();
        bad.capabilities.id = ProviderId::Boat;
        sqlx::query("INSERT INTO cloud_managed_bases VALUES('boxd','bad','verified',$1,1)")
            .bind(serde_json::to_value(bad).unwrap())
            .execute(&pool)
            .await
            .unwrap();
        assert!(
            store
                .managed_launch_catalog(&[connection.clone()])
                .await
                .is_err()
        );
        sqlx::query("UPDATE cloud_managed_bases SET provider='boat',runtime_manifest=$1")
            .bind(serde_json::to_value(release()).unwrap())
            .execute(&pool)
            .await
            .unwrap();
        assert!(store.managed_launch_catalog(&[connection]).await.is_err());
        pool.close().await;
        sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
            .execute(&admin)
            .await
            .unwrap();
        admin.close().await;
    }

    #[test]
    fn certification_cannot_accept_an_account_image_or_missing_capabilities() {
        let original = release();
        original.validate().unwrap();
        for mutate in [
            |r: &mut ManagedBaseRelease| r.manifest.managed_base = false,
            |r: &mut ManagedBaseRelease| r.manifest.credentials_removed = false,
            |r: &mut ManagedBaseRelease| r.manifest.rust_repository_fetch_version = None,
            |r: &mut ManagedBaseRelease| r.certification_report_sha256.clear(),
            |r: &mut ManagedBaseRelease| r.harnesses[0].sha256.clear(),
            |r: &mut ManagedBaseRelease| r.harnesses[0].catalog.default_model = "missing".into(),
            |r: &mut ManagedBaseRelease| r.harnesses[0].catalog.id = zuse_protocol::HarnessId::Grok,
            |r: &mut ManagedBaseRelease| r.harnesses.push(r.harnesses[0].clone()),
            |r: &mut ManagedBaseRelease| r.capabilities.configured = false,
            |r: &mut ManagedBaseRelease| r.capabilities.id = ProviderId::Boat,
        ] {
            let mut changed = original.clone();
            mutate(&mut changed);
            assert!(changed.validate().is_err());
        }
    }
}

#[cfg(test)]
#[path = "managed_bases/admission.rs"]
mod admission;

#[cfg(test)]
#[path = "managed_bases/organization.rs"]
mod organization_tests;
