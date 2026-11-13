//! Runtime bootstrap preserves original storage identity and durable enrollment receipts.
//! Only explicitly Rust-owned workspaces admit writes; no provider operations run here.
use crate::{
    crypto::{MasterKey, credential_thumbprint, seal_runtime_secret},
    error::ApiError,
    tickets::{
        GATEWAY_PROTOCOL, RUNTIME_TTL_MS, TicketSigner, signing_thumbprint, verify_renewal_proof,
    },
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{
    Connection, PgPool, Postgres, Transaction,
    postgres::{PgConnectOptions, PgPoolOptions},
};
use std::{env, str::FromStr, sync::Arc, time::Duration};
use subtle::ConstantTimeEq;

pub(crate) mod activity;
mod admission_batch;
pub mod allocation;
mod allocation_scope;
pub mod assets;
pub mod creation;
pub mod deletion;
mod effects;
mod fast_start;
pub mod github;
mod grant_receipts;
pub mod grants;
mod internal_observations;
pub mod keep_running;
pub mod lifecycle;
pub(crate) mod machine_layout;
pub mod mailbox;
mod object_writes;
pub mod preparation;
mod project_preparation;
pub mod provider;
pub mod recovery;
pub mod recreation;
pub mod repair;
pub mod restore;
pub mod runtime_updates;
pub mod scope;
mod startup_binding;
mod statement_batch;
pub mod task_commands;
pub mod task_history;
pub mod tool_output;
pub mod transcript;
mod worker_scope;
pub mod workload_secrets;

pub struct RuntimeConfig {
    staging_candidate: Option<Arc<crate::managed_bases::candidate::StagingBasePin>>,
    master: MasterKey,
    signer: TicketSigner,
    command_admission_creation: bool,
    idle_ms: i64,
    creation_accounts: Vec<String>,
    legacy_auth_url: Option<String>,
    recovery_enabled: bool,
    recovery_workspaces: Vec<String>,
}
impl RuntimeConfig {
    pub fn from_env() -> anyhow::Result<Option<Self>> {
        if env::var("ZUSE_CLOUD_RUNTIME_WRITES").as_deref() != Ok("true") {
            return Ok(None);
        }
        let encryption = env::var("CLOUD_DATA_ENCRYPTION_KEY")
            .or_else(|_| env::var("CLOUD_CREDENTIAL_VAULT_KEY"))
            .map_err(|_| anyhow::anyhow!("Runtime writes require CLOUD_DATA_ENCRYPTION_KEY"))?;
        let mint = env::var("RELAY_MINT_PRIVATE_JWK")
            .map_err(|_| anyhow::anyhow!("Runtime writes require RELAY_MINT_PRIVATE_JWK"))?;
        let issuer = env::var("API_ISSUER").unwrap_or_else(|_| "https://api-rust.zuse.sh".into());
        let idle_ms = env::var("CLOUD_WORKSPACE_IDLE_TIMEOUT_MS")
            .ok()
            .map(|s| s.parse::<i64>())
            .transpose()
            .map_err(|_| anyhow::anyhow!("Invalid cloud idle timeout"))?
            .unwrap_or(10 * 60 * 1000);
        anyhow::ensure!(
            (1000..=86400000).contains(&idle_ms),
            "Invalid cloud idle timeout"
        );
        let creation_accounts: Vec<String> = env::var("ZUSE_CLOUD_CREATE_ACCOUNTS")
            .unwrap_or_default()
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .collect();
        let staging_candidate =
            crate::managed_bases::candidate::StagingBasePin::from_environment(&creation_accounts)?;
        Ok(Some(Self {
            staging_candidate,
            legacy_auth_url: env::var("ZUSE_CLOUD_LEGACY_AUTH_URL").ok(),
            creation_accounts,
            recovery_enabled: env::var("CLOUD_WORKSPACE_RECOVERY_ENABLED").as_deref() == Ok("true"),
            recovery_workspaces: env::var("CLOUD_WORKSPACE_RECOVERY_WORKSPACE_IDS")
                .unwrap_or_default()
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .collect(),
            command_admission_creation: match env::var("ZUSE_CLOUD_COMMAND_ADMISSION_V2") {
                Err(env::VarError::NotPresent) => false,
                Ok(v) if v == "true" => true,
                Ok(v) if v == "false" => false,
                _ => anyhow::bail!("Invalid ZUSE_CLOUD_COMMAND_ADMISSION_V2"),
            },
            idle_ms,
            master: MasterKey::parse(&encryption)
                .map_err(|_| anyhow::anyhow!("Invalid cloud data encryption key"))?,
            signer: TicketSigner::from_jwk(&mint, &issuer)
                .map_err(|_| anyhow::anyhow!("Invalid runtime ticket signing configuration"))?,
        }))
    }
}

pub struct RuntimeControl {
    staging_candidate: Option<Arc<crate::managed_bases::candidate::StagingBasePin>>,
    scope_authority: std::sync::OnceLock<Arc<scope::ScopeAuthority>>,
    pool: PgPool,
    admission_batch_schema: tokio::sync::OnceCell<bool>,
    /// Set once the attachment release schema is seen; never cached as absent.
    asset_release_schema: std::sync::atomic::AtomicBool,
    startup_schema: tokio::sync::OnceCell<(bool, bool, bool)>,
    pub(crate) project_cache_inflight: std::sync::atomic::AtomicBool,
    pub(crate) project_cache_control: crate::project_caches::worker::CacheControl,
    task_command_schema_ready: tokio::sync::OnceCell<bool>,
    command_admission_creation: bool,
    recreation_schema_ready: tokio::sync::OnceCell<bool>,
    fast_start_accounts: Vec<String>,
    master: MasterKey,
    signer: TicketSigner,
    idle_ms: i64,
    creation_accounts: Vec<String>,
    grant_issuer: Option<Arc<dyn grants::GrantIssuer>>,
    authority: Option<Arc<crate::connections::ConnectionControl>>,
    transcript_objects: Option<Arc<dyn crate::objects::Objects>>,
    recovery_enabled: bool,
    recovery_workspaces: Vec<String>,
    object_writes: Arc<tokio::sync::Semaphore>,
    pub(crate) mailbox_changes: tokio::sync::broadcast::Sender<()>,
    runtime_releases: runtime_updates::RuntimeReleaseConfig,
}

pub use zuse_protocol::cloud::{
    CloudRuntimeBootstrapAck as BootstrapAck, CloudRuntimeBootstrapRequest as BootstrapRequest,
    CloudRuntimeCredentialRenewRequest as RenewRequest,
};

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Receipt {
    workspace_id: String,
    boot_token_hash: String,
    credential_key_thumbprint: String,
    signing_key_thumbprint: String,
    signing_public_jwk: String,
    runtime_credential_hash: String,
    runtime_credential_expires_at_ms: i64,
    generation: i64,
    gateway_epoch: i64,
    sealed_transcript_key: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    capabilities: Option<Vec<String>>,
    enrolled_at_ms: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    acknowledged_at_ms: Option<i64>,
}

impl RuntimeControl {
    pub fn staging_base_pin(&self) -> Option<Arc<crate::managed_bases::candidate::StagingBasePin>> {
        self.staging_candidate.clone()
    }
    pub fn with_staging_base_pin(
        mut self,
        pin: Option<Arc<crate::managed_bases::candidate::StagingBasePin>>,
    ) -> Self {
        self.staging_candidate = pin;
        self
    }
    pub(crate) async fn candidate_row_allowed(
        &self,
        row: &Value,
        now: i64,
    ) -> Result<(), ApiError> {
        if let Some(expected) = row["request_config"]["stagingProjectCachePilotSha256"].as_str() {
            let authority = crate::project_caches::pilot::current(now)?
                .ok_or_else(|| ApiError::forbidden("staging_project_cache_pilot_fenced"))?;
            if authority.pilot.fingerprint()? != expected
                || row["account_id"] != authority.pilot.owner_id
                || row["project_id"] != authority.pilot.project_id
                || row["request_config"]["rustActorId"] != authority.pilot.actor_id
            {
                return Err(ApiError::forbidden("staging_project_cache_pilot_fenced"));
            }
        }
        if row["request_config"]["managedCandidateSha256"].is_null() {
            return Ok(());
        }
        let pin = self
            .staging_candidate
            .as_ref()
            .ok_or_else(|| ApiError::forbidden("staging_managed_base_candidate_not_selected"))?;
        if self.statement_batch_available().await? {
            return self.candidate_row_allowed_batched(pin, row, now).await;
        }
        pin.row_allowed(&self.pool, row, now).await
    }
    pub fn with_credential_authority(
        mut self,
        authority: Arc<crate::connections::ConnectionControl>,
    ) -> Self {
        self.authority = Some(authority);
        self
    }
    pub async fn connect(url: &str, config: RuntimeConfig) -> anyhow::Result<Arc<Self>> {
        let options = PgConnectOptions::from_str(url)
            .map_err(|_| anyhow::anyhow!("Invalid cloud PostgreSQL configuration"))?
            .application_name("zuse-cloud-runtime-control");
        let fast_start = env::var("ZUSE_CLOUD_FAST_START_ACCOUNTS")
            .is_ok_and(|accounts| !accounts.trim().is_empty());
        let pool = PgPoolOptions::new()
            .max_connections(if fast_start { 8 } else { 4 })
            .min_connections(if fast_start { 2 } else { 0 })
            .test_before_acquire(!fast_start)
            .before_acquire(move |connection, metadata| {
                Box::pin(async move {
                    // A recently used connection need not pay another network RTT
                    // before every query. Idle connections still get a liveness test;
                    // durable transaction/effect fencing handles failures during use.
                    if fast_start && metadata.idle_for >= Duration::from_secs(15) {
                        connection.ping().await?;
                    }
                    Ok(true)
                })
            })
            .acquire_timeout(Duration::from_secs(5))
            .after_connect(|c, _| {
                Box::pin(async move {
                    sqlx::query("SET statement_timeout = '10s'")
                        .execute(c)
                        .await?;
                    Ok(())
                })
            })
            .connect_with(options)
            .await
            .map_err(|_| anyhow::anyhow!("Cannot connect runtime-control database"))?;
        let mut control = Self::from_pool(pool, config.master, config.signer);
        control.transcript_objects = crate::objects::R2::from_env()?;
        control.runtime_releases = runtime_updates::RuntimeReleaseConfig::from_env()?;
        control.idle_ms = config.idle_ms;
        control.command_admission_creation = config.command_admission_creation;
        control.recovery_enabled = config.recovery_enabled;
        control.recovery_workspaces = config.recovery_workspaces;
        control.creation_accounts = config.creation_accounts;
        control.fast_start_accounts = env::var("ZUSE_CLOUD_FAST_START_ACCOUNTS")
            .unwrap_or_default()
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .collect();
        anyhow::ensure!(
            control.fast_start_accounts.is_empty()
                || env::var("ZUSE_DEPLOYMENT_ENVIRONMENT").as_deref() == Ok("staging"),
            "Fast startup is restricted to staging"
        );
        control.staging_candidate = config.staging_candidate;
        let auth_accounts = env::var("ZUSE_CLOUD_AUTH_WRITE_ACCOUNTS")
            .unwrap_or_default()
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .collect::<Vec<_>>();
        if !auth_accounts.is_empty() {
            let encryption = env::var("CLOUD_DATA_ENCRYPTION_KEY")
                .or_else(|_| env::var("CLOUD_CREDENTIAL_VAULT_KEY"))?;
            let master = MasterKey::parse(&encryption)
                .map_err(|_| anyhow::anyhow!("Invalid credential authority encryption key"))?;
            control.authority = Some(Arc::new(crate::connections::ConnectionControl::from_pool(
                control.pool.clone(),
                master,
                auth_accounts,
            )));
        }
        if let Some(url) = config.legacy_auth_url {
            control.grant_issuer = Some(Arc::new(
                grants::LegacyGrantIssuer::new(&url, &control.signer.issuer())
                    .map_err(|error| anyhow::anyhow!("{}", error.code))?,
            ));
        }
        control
            .ready()
            .await
            .map_err(|error| anyhow::anyhow!("Runtime-control is not ready ({})", error.code))?;
        control
            .statement_batch_available()
            .await
            .map_err(|error| anyhow::anyhow!(error.code))?;
        Ok(Arc::new(control))
    }
    pub fn from_pool(pool: PgPool, master: MasterKey, signer: TicketSigner) -> Self {
        Self {
            project_cache_inflight: std::sync::atomic::AtomicBool::new(false),
            project_cache_control: crate::project_caches::worker::CacheControl::from_pool(
                pool.clone(),
            ),
            scope_authority: std::sync::OnceLock::new(),
            recreation_schema_ready: tokio::sync::OnceCell::new(),
            admission_batch_schema: tokio::sync::OnceCell::new(),
            asset_release_schema: std::sync::atomic::AtomicBool::new(false),
            startup_schema: tokio::sync::OnceCell::new(),
            task_command_schema_ready: tokio::sync::OnceCell::new(),
            command_admission_creation: false,
            fast_start_accounts: vec![],
            pool,
            master,
            signer,
            idle_ms: 10 * 60 * 1000,
            recovery_enabled: false,
            recovery_workspaces: vec![],
            creation_accounts: vec![],
            staging_candidate: None,
            grant_issuer: None,
            authority: None,
            transcript_objects: None,
            object_writes: Arc::new(tokio::sync::Semaphore::new(1)),
            mailbox_changes: tokio::sync::broadcast::channel(128).0,
            runtime_releases: runtime_updates::RuntimeReleaseConfig::default(),
        }
    }
    pub async fn ready(&self) -> Result<(), ApiError> {
        self.statement_batch_available().await?;
        if let Some(authority) = &self.authority {
            authority.ready().await?;
            let script = std::env::var_os("ZUSE_CLOUD_AUTH_DRIVER")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| std::path::PathBuf::from("/usr/local/lib/zuse/auth-driver.py"));
            crate::connections::worker::CommandDriver::deployment_ready(&script)
                .map_err(|_| ApiError::unavailable("cloud_auth_tools_not_ready"))?;
        }
        let ready: bool = sqlx::query_scalar("SELECT to_regclass('zuse_rust_control_owners') IS NOT NULL AND to_regclass('zuse_rust_provider_grant_receipts') IS NOT NULL AND to_regclass('api_cloud_workspace_launch_intents') IS NOT NULL AND to_regclass('zuse_rust_gateway_leases') IS NOT NULL AND to_regclass('zuse_rust_mailbox_commands') IS NOT NULL AND to_regclass('zuse_rust_lifecycle_operations') IS NOT NULL AND to_regclass('zuse_rust_deletions') IS NOT NULL AND to_regclass('zuse_rust_provider_creates') IS NOT NULL AND EXISTS(SELECT 1 FROM information_schema.columns WHERE table_schema=current_schema() AND table_name='zuse_rust_lifecycle_operations' AND column_name='provider_effects')")
            .fetch_one(&self.pool).await?;
        if !ready {
            return Err(ApiError::unavailable("cloud_runtime_schema_not_ready"));
        }
        if self.recovery_enabled {
            self.recovery_ready().await?;
        }
        if self.transcript_objects.is_some() {
            let ready: bool = sqlx::query_scalar(
                "SELECT to_regclass('api_cloud_transcript_checkpoints') IS NOT NULL",
            )
            .fetch_one(&self.pool)
            .await?;
            if !ready {
                return Err(ApiError::unavailable("cloud_transcript_schema_not_ready"));
            }
        }
        if self.grant_issuer.is_some() {
            let ready: bool =
                sqlx::query_scalar("SELECT to_regclass('api_cloud_auth_authorities') IS NOT NULL")
                    .fetch_one(&self.pool)
                    .await?;
            if !ready {
                return Err(ApiError::unavailable(
                    "cloud_auth_authority_schema_not_ready",
                ));
            }
        }
        self.task_command_delivery_available().await?;
        if self.creation_enabled() {
            self.admission_batch_available().await?;
            if self.command_admission_creation && !self.task_command_creation_available().await? {
                return Err(ApiError::unavailable("cloud_task_command_schema_not_ready"));
            }
            self.allocation_ready().await?;
            let ready:bool=sqlx::query_scalar("SELECT to_regclass('api_cloud_projects') IS NOT NULL AND to_regclass('api_cloud_project_builds') IS NOT NULL AND to_regclass('api_cloud_snapshots') IS NOT NULL AND to_regclass('api_cloud_billing_periods') IS NOT NULL AND to_regclass('api_cloud_billing_ledger') IS NOT NULL AND to_regclass('api_cloud_billing_reservations') IS NOT NULL").fetch_one(&self.pool).await?;
            if !ready {
                return Err(ApiError::unavailable("cloud_creation_schema_not_ready"));
            }
        }
        Ok(())
    }

    pub fn credential_authority(&self) -> Option<Arc<crate::connections::ConnectionControl>> {
        self.authority.clone()
    }
    pub fn shared_pool(&self) -> PgPool {
        self.pool.clone()
    }

    #[tracing::instrument(name="cloud_runtime_request", skip_all, fields(workspace=%id, operation="bootstrap"))]
    pub async fn bootstrap(
        &self,
        id: &str,
        token: &str,
        request: BootstrapRequest,
        now: i64,
    ) -> Result<Value, ApiError> {
        if token.is_empty()
            || token.len() > 512
            || request
                .capabilities
                .as_ref()
                .is_some_and(|caps| caps.len() > 128 || caps.iter().any(|s| s.len() > 128))
        {
            return Err(rejected());
        }
        let credential_thumbprint = credential_thumbprint(&request.credential_public_jwk)
            .map_err(|_| ApiError::bad_request("invalid_workspace_key"))?;
        let signing_thumbprint = signing_thumbprint(&request.signing_public_jwk)?;
        let token_hash = hash(token);
        let early: Value = sqlx::query_scalar(
            "SELECT to_jsonb(w) FROM api_cloud_workspaces w WHERE workspace_id=$1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(rejected)?;
        if early["runtime_boot_token_hash"]
            .as_str()
            .is_none_or(|h| !equal(h, &token_hash))
            || early["runtime_boot_token_expires_at"].as_i64().unwrap_or(0) <= now
            || early["desired_state"] != "ready"
            || early["provider_sandbox_id"].as_str().is_none()
            || blocked(&early)
        {
            return Err(rejected());
        }
        if early["account_id"]
            .as_str()
            .is_some_and(|owner| owner.starts_with("organization:"))
        {
            if !scope::native_organization_task(&early) {
                return Err(ApiError::conflict("workspace_runtime_update_required"));
            }
            self.scope_access(id, None, true).await?;
        }
        let internal_observations = self
            .internal_codex_tool_observations(
                &early,
                env::var("ZUSE_DEPLOYMENT_ENVIRONMENT").ok().as_deref(),
                env::var("ZUSE_CLOUD_INTERNAL_CODEX_OBSERVATION_OWNER")
                    .ok()
                    .as_deref(),
                now,
            )
            .await;
        let internal_memory_transport = self
            .internal_codex_tool_observations(
                &early,
                env::var("ZUSE_DEPLOYMENT_ENVIRONMENT").ok().as_deref(),
                env::var("ZUSE_CLOUD_INTERNAL_CODEX_MEMORY_TRANSPORT_OWNER")
                    .ok()
                    .as_deref(),
                now,
            )
            .await;
        let mut tx = self.pool.begin().await?;
        let (row, generation, epoch) = locked_workspace(&mut tx, id).await?;
        if row["runtime_boot_token_hash"]
            .as_str()
            .is_none_or(|h| !equal(h, &token_hash))
            || row["runtime_boot_token_expires_at"].as_i64().unwrap_or(0) <= now
            || row["desired_state"] != "ready"
            || row["provider_sandbox_id"].as_str().is_none()
            || blocked(&row)
        {
            return Err(rejected());
        }
        let owner = required(&row, "account_id")?;
        let mut config = row["request_config"].clone();
        let rpc_protocol = negotiate_rpc(&config, request.rpc_protocol.as_deref())?.to_owned();
        crate::project_caches::require_runtime_capability(&config, request.capabilities.as_ref())?;
        if owner.starts_with("organization:")
            && !request.capabilities.as_ref().is_some_and(|caps| {
                caps.iter()
                    .any(|cap| cap == "rust-organization-execution-v1")
            })
        {
            return Err(ApiError::conflict("workspace_runtime_update_required"));
        }
        if config["initialGithubContext"]["slackMessageId"].is_string()
            && !request
                .capabilities
                .as_ref()
                .is_some_and(|caps| caps.iter().any(|cap| cap == "github-execution-v1"))
        {
            return Err(ApiError::conflict("github_runtime_update_required"));
        }
        let runtime_credential = format!(
            "workspace_runtime_{}",
            hash(&format!(
                "bootstrap-v1\n{token}\n{id}\n{generation}\n{epoch}\n{credential_thumbprint}\n{signing_thumbprint}"
            ))
        );
        let runtime_hash = hash(&runtime_credential);
        let prior = config.get("runtimeBootstrapReceipt");
        let receipt = if let Some(prior) = prior {
            let receipt: Receipt = serde_json::from_value(prior.clone()).map_err(|_| rejected())?;
            if receipt.workspace_id != id
                || !equal(&receipt.boot_token_hash, &token_hash)
                || receipt.credential_key_thumbprint != credential_thumbprint
                || receipt.signing_key_thumbprint != signing_thumbprint
                || receipt.generation != generation
                || receipt.gateway_epoch != epoch
                || !equal(&receipt.runtime_credential_hash, &runtime_hash)
                || receipt.runtime_credential_expires_at_ms <= now
                || row["runtime_credential_hash"]
                    .as_str()
                    .is_none_or(|h| !equal(h, &runtime_hash))
            {
                return Err(ApiError::unauthorized("workspace_bootstrap_key_mismatch"));
            }
            receipt
        } else {
            let (key, envelope) = match row["wrapped_transcript_key"].as_str() {
                Some(envelope) => (
                    self.master
                        .open_transcript_key(owner, id, envelope)
                        .map_err(|_| ApiError::unavailable("cloud_transcript_key_unavailable"))?,
                    envelope.to_owned(),
                ),
                None => self
                    .master
                    .create_transcript_key(owner, id)
                    .map_err(|_| ApiError::unavailable("cloud_transcript_key_unavailable"))?,
            };
            let sealed = seal_runtime_secret(&request.credential_public_jwk, &key)
                .map_err(|_| ApiError::unavailable("cloud_transcript_key_unavailable"))?;
            let receipt = Receipt {
                workspace_id: id.into(),
                boot_token_hash: token_hash,
                credential_key_thumbprint: credential_thumbprint.clone(),
                signing_key_thumbprint: signing_thumbprint.clone(),
                signing_public_jwk: request.signing_public_jwk.clone(),
                runtime_credential_hash: runtime_hash,
                runtime_credential_expires_at_ms: now + RUNTIME_TTL_MS,
                generation,
                gateway_epoch: epoch,
                sealed_transcript_key: sealed,
                capabilities: request.capabilities,
                enrolled_at_ms: now,
                acknowledged_at_ms: None,
            };
            config["runtimeGeneration"] = json!(generation);
            config["gatewayEpoch"] = json!(epoch);
            config["runtimeSigningPublicJwk"] = json!(receipt.signing_public_jwk);
            config["runtimeSigningKeyThumbprint"] = json!(signing_thumbprint);
            config["runtimeCredentialKeyThumbprint"] = json!(credential_thumbprint);
            config["runtimeCredentialExpiresAtMs"] =
                json!(receipt.runtime_credential_expires_at_ms);
            config["runtimeBootstrapReceipt"] =
                serde_json::to_value(&receipt).map_err(|_| rejected())?;
            if self
                .fast_start_accounts
                .iter()
                .any(|account| account == owner)
                && rpc_protocol == zuse_protocol::cloud_gateway::RUST_RPC
            {
                startup_binding::prepare(&mut config, &row, generation, now)?;
            }
            let mut timings = config
                .get("startupTimings")
                .filter(|v| v.is_object())
                .cloned()
                .unwrap_or_else(|| json!({}));
            timings["enrolledAt"] = json!(now);
            timings["bootstrapIssuedAt"] = json!(now);
            timings["runtimeReadyAt"] = json!(now);
            if let Some(allocated) = timings["allocatedAt"].as_i64() {
                timings["enrollmentDurationMs"] = json!(now.saturating_sub(allocated));
                timings["allocationToBootstrapMs"] = json!(now.saturating_sub(allocated));
            }
            config["startupTimings"] = timings;
            let next = if config["runtimeReleaseChangeRequested"] == true
                || matches!(
                    config["runtimeActivation"]["phase"].as_str(),
                    Some("preparing" | "launching" | "confirming" | "rolling-back")
                ) {
                row["next_action_at"]
                    .as_i64()
                    .unwrap_or(now)
                    .min(now + 30000)
            } else {
                now + 30000
            };
            sqlx::query("UPDATE api_cloud_workspaces SET wrapped_transcript_key=$2,runtime_credential_hash=$3,runtime_state='connecting',state='setup',status_code='runtime-authenticating',request_config=$4,next_action_at=$5,revision=revision+1,updated_at=GREATEST(updated_at+1,$6) WHERE workspace_id=$1")
                .bind(id).bind(envelope).bind(&receipt.runtime_credential_hash).bind(&config).bind(next).bind(now).execute(&mut *tx).await?;
            receipt
        };
        // No launch content is committed to a replacement conversation on missing/expired state.
        let intent: Option<Value> = sqlx::query_scalar("SELECT to_jsonb(i) FROM api_cloud_workspace_launch_intents i WHERE workspace_id=$1 AND expires_at>$2")
            .bind(id).bind(now).fetch_optional(&mut *tx).await?;
        let intent = match intent {
            Some(intent) => {
                if intent["account_id"] != owner
                    || intent["chat_id"] != row["chat_id"]
                    || intent["session_id"] != row["initial_session_id"]
                {
                    return Err(ApiError::conflict("cloud_workspace_launch_failed"));
                }
                let plaintext = self
                    .master
                    .open(
                        format!("zuse-cloud-workspace-launch-intent-v1\n{owner}\n{id}").as_bytes(),
                        required(&intent, "ciphertext")?,
                    )
                    .map_err(|_| {
                        ApiError::unavailable("cloud_workspace_launch_intent_unavailable")
                    })?;
                let content: Value = serde_json::from_slice(&plaintext)
                    .map_err(|_| ApiError::conflict("cloud_workspace_launch_failed"))?;
                let decoded: zuse_protocol::cloud::CloudLaunchIntent =
                    serde_json::from_value(content)
                        .map_err(|_| ApiError::conflict("cloud_workspace_launch_failed"))?;
                if decoded.command_id != intent["command_id"]
                    || decoded.turn_id != intent["turn_id"]
                {
                    return Err(ApiError::conflict("cloud_workspace_launch_failed"));
                }
                Some(
                    serde_json::to_value(decoded)
                        .map_err(|_| ApiError::conflict("cloud_workspace_launch_failed"))?,
                )
            }
            None if config["sessionHeadVersion"].is_number()
                || row["status_code"] == "agent-starting" =>
            {
                None
            }
            None => return Err(ApiError::conflict("cloud_workspace_launch_failed")),
        };
        let gateway_credential = self.signer.runtime_with_rpc(
            owner,
            id,
            generation,
            epoch,
            receipt.runtime_credential_expires_at_ms,
            &rpc_protocol,
        )?;
        let scope = owner
            .strip_prefix("organization:")
            .map(|org| json!({"kind":"organization","organizationId":org}))
            .unwrap_or_else(|| json!({"kind":"personal"}));
        let mut response = json!({"workspaceId":id,"zuseAccountId":owner,"workspaceScope":scope,"providerSandboxId":row["provider_sandbox_id"],
            "runtimeCredential":runtime_credential,"runtimeGatewayCredential":gateway_credential,"cloudCredentials":[],
            "gatewayUrl":self.signer.gateway_url(id),"gatewayProtocol":if rpc_protocol==zuse_protocol::cloud_gateway::RUST_RPC {zuse_protocol::cloud_gateway::RUST_PROTOCOL} else {GATEWAY_PROTOCOL},"rpcProtocol":rpc_protocol,"chatId":row["chat_id"],"initialSessionId":row["initial_session_id"],
            "codexAuthMode":auth_mode(&config["codexAuthMode"]),"providerAuthMode":auth_mode(&config["providerAuthMode"]),
            "runtimeGeneration":generation,"gatewayEpoch":epoch,"runtimeCredentialExpiresAt":receipt.runtime_credential_expires_at_ms,
            "sealedTranscriptKey":receipt.sealed_transcript_key});
        response["actorId"] = config["rustActorId"]
            .as_str()
            .or_else(|| (!owner.starts_with("organization:")).then_some(owner))
            .map(Value::from)
            .unwrap_or(Value::Null);
        if task_commands::command_mode(&row) {
            let (policy, _) = self.task_policy_tx(&mut tx, &row).await?;
            let mut catalogs = policy.catalogs.clone();
            let initial = if policy.agent == "claude-code" {
                "claude"
            } else {
                &policy.agent
            };
            catalogs.entry(initial.into()).or_insert(policy.catalog);
            response["managedAgentCatalogs"] = serde_json::to_value(catalogs)
                .map_err(|_| ApiError::unavailable("cloud_task_model_catalog_missing"))?;
        }
        if let Some(intent) = intent {
            response["launchIntent"] = intent;
        }
        if rpc_protocol == zuse_protocol::cloud_gateway::RUST_RPC {
            if config["rustFastStartVersion"] == 1
                && receipt
                    .capabilities
                    .as_ref()
                    .is_some_and(|caps| caps.iter().any(|c| c == "rust-fast-start-v1"))
            {
                response["rustFastStartVersion"] = json!(1);
                response["bootstrapAckMode"] = json!("gateway-v1");
                response["preparationIngressVersion"] = json!(1);
            }
            if startup_binding::negotiated(&config, &row, generation) {
                response["startupBindingVersion"] = json!(2);
                response["storageEnrollmentId"] =
                    config["rustStorageEnrollment"]["enrollmentId"].clone();
            }
            response["internalCodexToolObservations"] =
                json!(internal_observations && internal_observations::same_binding(&early, &row));
            response["internalCodexMemoryTransportReset"] = json!(
                internal_memory_transport && internal_observations::same_binding(&early, &row)
            );
            if config["agent"] == "codex" && config["managedBaseReadiness"] == "uncertified-staging"
            {
                let admission = lifecycle::memory_authority::pilot_admission(
                    &row,
                    response["internalCodexMemoryTransportReset"] == true,
                );
                sqlx::query("UPDATE api_cloud_workspaces SET request_config=jsonb_set(request_config,'{rustMemoryTransportAdmission}',$2) WHERE workspace_id=$1")
                    .bind(id).bind(admission).execute(&mut *tx).await?;
            }

            if config["rustOrganizationExecutionVersion"] == 1 {
                response["rustOrganizationExecutionVersion"] = json!(1);
            }
            if config["gitAuthMode"] == "broker-v1" {
                response["gitAuthMode"] = json!("broker-v1");
            }
            if let Some(agent) = config.get("agent").filter(|v| v.is_string()) {
                response["agent"] = agent.clone();
            }
            response["commandMailboxEnabled"] = json!(config["rustCommandMailboxEnabled"] == true);
            if config["commandDeliveryMode"] == zuse_protocol::cloud_task_commands::MODE
                && config["rustCommandAdmissionVersion"]
                    == zuse_protocol::cloud_task_commands::VERSION
                && config["rustCommandMailboxEnabled"] == true
            {
                response["commandDeliveryMode"] = json!(zuse_protocol::cloud_task_commands::MODE);
                response["commandAdmissionVersion"] =
                    json!(zuse_protocol::cloud_task_commands::VERSION);
                response["commandPolicyRevision"] = json!(1);
            }
            response["nativeTranscriptProjectionEnabled"] = json!(
                self.transcript_objects.is_some() && config["rustNativeTranscriptEnabled"] == true
            );
            if let Some(plan) = config.get("rustRepositoryCheckout") {
                response["rustRepositoryCheckout"] = plan.clone();
            }
            if config["rustGithubCliVersion"] == 1 {
                response["rustGithubCliVersion"] = json!(1);
            }
            if recreation_bootstrap_supported(&row, &receipt) {
                response["rustRecreationVersion"] = json!(1);
            }
            if config["rustMemorySleepVersion"] == 1 {
                response["rustMemorySleepVersion"] = json!(1);
            }
            if config["rustPreparationVersion"] == 1 {
                response["rustPreparationVersion"] = json!(1);
                if config["rustToolOutputVersion"] == 1 {
                    response["rustToolOutputVersion"] = json!(1);
                }
                response["rustAllocationOperationId"] = config["rustAllocationOperationId"].clone();
            }
            if config["rustWorkloadSecretsVersion"] == 1 {
                response["rustWorkloadSecretsVersion"] = json!(1);
                response["workloadSecretBindings"] = config["workloadSecretBindings"].clone();
            }
            if let Some(cache) = config.get("rustProjectCache") {
                response["rustProjectCache"] = cache.clone();
            }
            if let Some(recipe) = config.get("environmentRecipe") {
                response["environmentRecipe"] = recipe.clone();
                response["environmentRevision"] = config["environmentRevision"].clone();
            }
            // The account's latest machine instructions, skills and files,
            // read at each enrollment so a new chat gets the newest version.
            let delivers_machine = receipt.capabilities.as_ref().is_some_and(|caps| {
                caps.iter()
                    .any(|c| c == zuse_protocol::cloud_computers::MACHINE_DELIVERY_CAPABILITY)
            });
            if delivers_machine
                && let Some(delivery) = crate::computers::machine::delivery(&mut tx, owner).await?
            {
                response["machineDelivery"] = serde_json::to_value(delivery)
                    .map_err(|_| ApiError::unavailable("machine_record_invalid"))?;
            }
            let storage: zuse_protocol::cloud::CloudRuntimeStorage =
                serde_json::from_value(config["rustRuntimeStorage"].clone())
                    .map_err(|_| ApiError::conflict("runtime_storage_enrollment_required"))?;
            if storage.format != zuse_protocol::cloud::RUST_RUNTIME_STORAGE_FORMAT
                || storage.storage_incarnation_id.is_empty()
                || storage.storage_incarnation_id.len() > 128
            {
                return Err(ApiError::conflict("runtime_storage_enrollment_required"));
            }
            if let Some(grant) = config.get("rustRecoveryRestore") {
                self.validate_restore_enrollment(&mut tx, &row, generation)
                    .await?;
                response["rustRecoveryRestore"] = grant.clone();
            }
            response["runtimeStorage"] = serde_json::to_value(storage)
                .map_err(|_| ApiError::conflict("runtime_storage_enrollment_required"))?;
        }
        if let Some(context) = config.get("initialGithubContext") {
            response["initialGithubContext"] = context.clone();
        }
        tx.commit().await?;
        Ok(response)
    }

    #[tracing::instrument(name="cloud_runtime_request", skip_all, fields(workspace=%id, operation="enroll_storage"))]
    pub async fn enroll_storage(
        &self,
        id: &str,
        credential: &str,
        request: zuse_protocol::cloud::CloudStorageEnrollment,
        now: i64,
    ) -> Result<Value, ApiError> {
        self.runtime_scope_access(id, credential, now).await?;
        let mut tx = self.pool.begin().await?;
        let (row, generation, epoch) = locked_workspace(&mut tx, id).await?;
        if row["runtime_credential_hash"]
            .as_str()
            .is_none_or(|h| !equal(h, &hash(credential)))
            || row["request_config"]["runtimeCredentialExpiresAtMs"]
                .as_i64()
                .unwrap_or(0)
                <= now
        {
            return Err(ApiError::unauthorized("workspace_runtime_fenced"));
        }
        let (_, enrollment) = self
            .enroll_storage_tx(&mut tx, &row, generation, epoch, request, now)
            .await?;
        tx.commit().await?;
        Ok(json!({"enrollmentId":enrollment}))
    }

    async fn enroll_storage_tx(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        row: &Value,
        generation: i64,
        epoch: i64,
        request: zuse_protocol::cloud::CloudStorageEnrollment,
        now: i64,
    ) -> Result<(Value, String), ApiError> {
        use zuse_protocol::cloud::CloudStoragePhase;
        // Unknown names are kept: a newer runtime can know capabilities this
        // server does not (for example after a server rollback).
        if request
            .capabilities
            .as_ref()
            .is_some_and(|caps| !crate::runtime_releases::names_valid(caps))
        {
            return Err(ApiError::bad_request("invalid_runtime_capabilities"));
        }
        let id = required(row, "workspace_id")?;
        let mut config = row["request_config"].clone();
        if !enrollment_matches(&row)
            || blocked(&row)
            || row["desired_state"] != "ready"
            || request.runtime_generation != generation
            || request.gateway_epoch != epoch
        {
            return Err(ApiError::unauthorized("workspace_runtime_fenced"));
        }
        if config["runtimeRpcProtocol"] != zuse_protocol::cloud_gateway::RUST_RPC
            || config["rustRuntimeStorage"]["storageIncarnationId"]
                != request.storage_incarnation_id
            || request.storage_incarnation_id.is_empty()
        {
            return Err(ApiError::conflict("runtime_storage_enrollment_required"));
        }
        let credential_key = config["runtimeCredentialKeyThumbprint"]
            .as_str()
            .ok_or_else(rejected)?
            .to_owned();
        let signing_key = config["runtimeSigningKeyThumbprint"]
            .as_str()
            .ok_or_else(rejected)?
            .to_owned();
        let source = required(&row, "provider_sandbox_id")?;
        let prior = config.get("rustStorageEnrollment").cloned();
        let mut record = match prior {
            Some(prior) => {
                if prior["storageIncarnationId"] != request.storage_incarnation_id
                    || prior["credentialKeyThumbprint"] != credential_key
                    || prior["signingKeyThumbprint"] != signing_key
                    || prior["sourceSandboxId"] != source
                {
                    return Err(ApiError::conflict("runtime_storage_recovery_required"));
                }
                prior
            }
            None => {
                if request.phase != CloudStoragePhase::Prepare
                    || config["sessionHeadVersion"].is_number()
                        && config["rustRecoveryRestore"].is_null()
                {
                    return Err(ApiError::conflict("runtime_storage_recovery_required"));
                }
                if !config["rustRecoveryRestore"].is_null() {
                    self.validate_restore_enrollment(tx, row, generation)
                        .await?;
                }
                json!({"enrollmentId":uuid::Uuid::new_v4().to_string(),"phase":"prepared","storageIncarnationId":request.storage_incarnation_id,"credentialKeyThumbprint":credential_key,"signingKeyThumbprint":signing_key,"sourceSandboxId":source,"generation":generation,"preparedAt":now})
            }
        };
        let enrollment = record["enrollmentId"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| ApiError::conflict("runtime_storage_recovery_required"))?
            .to_owned();
        match request.phase {
            CloudStoragePhase::Prepare => {
                if record["phase"] != "prepared" {
                    return Err(ApiError::conflict("runtime_storage_recovery_required"));
                }
            }
            CloudStoragePhase::Commit => {
                crate::project_caches::require_runtime_capability(
                    &config,
                    request.capabilities.as_ref(),
                )?;
                if task_commands::command_mode(&row)
                    && request.capabilities.as_ref().is_none_or(|caps| {
                        !caps
                            .iter()
                            .any(|cap| cap == zuse_protocol::cloud_task_commands::CAPABILITY)
                    })
                {
                    return Err(ApiError::conflict("cloud_task_runtime_update_required"));
                }
                if request.enrollment_id.as_deref() != Some(&enrollment)
                    || !matches!(record["phase"].as_str(), Some("prepared" | "committed"))
                {
                    return Err(ApiError::conflict("runtime_storage_enrollment_rejected"));
                }
                record["capabilities"] = json!(request.capabilities.unwrap_or_default());
                record["phase"] = json!("committed");
                record["generation"] = json!(generation);
                if record["committedAt"].is_null() {
                    record["committedAt"] = json!(now);
                }
                if config["rustFastStartVersion"] == 1
                    && receipt_fast_start(&config)
                    && config["rustAllocationOperationId"]
                        .as_str()
                        .is_some_and(|id| !id.is_empty())
                {
                    config["rustPreparationIngress"] = json!({"version":1,"operationId":config["rustAllocationOperationId"],"generation":generation,"sourceMachineId":row["provider_sandbox_id"],"storageIncarnationId":config["rustRuntimeStorage"]["storageIncarnationId"],"connectedAt":now});
                }
            }
        }
        if config.get("rustStorageEnrollment") != Some(&record)
            || config.get("rustPreparationIngress")
                != row["request_config"].get("rustPreparationIngress")
        {
            config["rustStorageEnrollment"] = record;
            sqlx::query("UPDATE api_cloud_workspaces SET request_config=$2,revision=revision+1,updated_at=GREATEST(updated_at+1,$3) WHERE workspace_id=$1").bind(id).bind(&config).bind(now).execute(&mut **tx).await?;
        }
        let mut current = row.clone();
        current["request_config"] = config;
        Ok((current, enrollment))
    }

    #[tracing::instrument(name="cloud_runtime_request", skip_all, fields(workspace=%id, operation="ready_launch"))]
    pub async fn ready_launch(
        &self,
        id: &str,
        credential: &str,
        request: zuse_protocol::cloud::CloudRuntimeReady,
        now: i64,
    ) -> Result<Value, ApiError> {
        if !["agent-started", "agent-ready"].contains(&request.phase.as_str())
            || request.session_head_version.is_none_or(|v| v < 0)
        {
            return Err(ApiError::bad_request("invalid_runtime_ready_request"));
        }
        let command = request
            .launch_command_id
            .filter(|s| !s.is_empty() && s.len() <= 512)
            .ok_or_else(|| ApiError::bad_request("invalid_runtime_ready_request"))?;
        let head = request.session_head_version.unwrap_or(0);
        self.runtime_scope_access(id, credential, now).await?;
        let mut tx = self.pool.begin().await?;
        let (row, generation, _) = locked_workspace(&mut tx, id).await?;
        if !enrollment_matches(&row)
            || blocked(&row)
            || row["desired_state"] != "ready"
            || row["runtime_credential_hash"]
                .as_str()
                .is_none_or(|h| !equal(h, &hash(credential)))
            || row["request_config"]["runtimeCredentialExpiresAtMs"]
                .as_i64()
                .unwrap_or(0)
                <= now
        {
            return Err(ApiError::unauthorized("workspace_runtime_fenced"));
        }
        if row["request_config"]["runtimeRpcProtocol"] != zuse_protocol::cloud_gateway::RUST_RPC {
            return Err(ApiError::conflict("workspace_rpc_protocol_mismatch"));
        }
        let mut config = row["request_config"].clone();
        if !storage_enrolled(&row) {
            return Err(ApiError::conflict("runtime_storage_enrollment_required"));
        }

        if task_commands::command_mode(&row) && !native_commands_supported(&row) {
            return Err(ApiError::conflict("cloud_task_runtime_update_required"));
        }
        let intent:Option<String>=sqlx::query_scalar("SELECT command_id FROM api_cloud_workspace_launch_intents WHERE workspace_id=$1 FOR UPDATE").bind(id).fetch_optional(&mut *tx).await?;
        if intent.as_deref() != Some(&command)
            && !(intent.is_none() && config["rustCompletedLaunchCommandId"] == command)
        {
            return Err(ApiError::conflict("launch_intent_receipt_rejected"));
        }
        let previous = config["sessionHeadVersion"].as_i64().unwrap_or(0);
        if request.phase == "agent-ready" {
            if config["rustFastStartVersion"] != 1
                || !receipt_fast_start(&config)
                || config["rustPreparationIngress"]["operationId"]
                    != config["rustAllocationOperationId"]
            {
                return Err(ApiError::conflict("runtime_early_agent_not_admitted"));
            }
            config["rustAgentPreparationReady"] = json!(true);
            let mut proposed = row.clone();
            proposed["request_config"] = config.clone();
            if !Self::early_agent_ready(&proposed) {
                return Err(ApiError::conflict("runtime_early_agent_not_admitted"));
            }
            if config["startupTimings"]["agentReadyAt"].is_null() {
                config["startupTimings"]["agentReadyAt"] = json!(now);
            }
            sqlx::query("UPDATE api_cloud_workspaces SET runtime_state='online',request_config=$2,revision=revision+1,updated_at=GREATEST(updated_at+1,$3) WHERE workspace_id=$1")
                .bind(id).bind(&config).bind(now).execute(&mut *tx).await?;
            proposed["runtime_state"] = json!("online");
            proposed["request_config"] = config;
            Self::reconcile_abandoned_allocation(&mut tx, &proposed, generation, now).await?;
            tx.commit().await?;
            self.notify_task_drain();
            return Ok(json!({"agentReady":true,"workspaceReady":false}));
        }
        if intent.is_some() || head > previous {
            config["sessionHeadVersion"] = json!(previous.max(head));
            config["rustCompletedLaunchCommandId"] = json!(command);
            for key in [
                "runtimeSessionRecoveryPending",
                "launchErrorCode",
                "startupFailureDiagnostic",
            ] {
                config.as_object_mut().ok_or_else(rejected)?.remove(key);
            }
            if !config["startupTimings"].is_object() {
                config["startupTimings"] = json!({});
            }
            for key in ["connectedAt", "repositoryReadyAt"] {
                if config["startupTimings"][key].is_null() {
                    config["startupTimings"][key] = json!(now);
                }
            }
            if head > 0 && config["startupTimings"]["messageAcceptedAt"].is_null() {
                config["startupTimings"]["messageAcceptedAt"] = json!(now);
            }
            let mut next = now + self.idle_ms;
            if config["runtimeReleaseChangeRequested"] == true
                || matches!(
                    config["runtimeActivation"]["phase"].as_str(),
                    Some("preparing" | "launching" | "confirming" | "rolling-back")
                )
            {
                next = next.min(row["next_action_at"].as_i64().unwrap_or(next));
            }
            sqlx::query("UPDATE api_cloud_workspaces SET runtime_state='online',state='ready',status_code='ready',request_config=$2,revision=revision+1,updated_at=GREATEST(updated_at+1,$3),last_activity_at=$3,next_action_at=$4 WHERE workspace_id=$1")
                .bind(id).bind(config).bind(now).bind(next).execute(&mut *tx).await?;
            sqlx::query("DELETE FROM api_cloud_workspace_launch_intents WHERE workspace_id=$1")
                .bind(id)
                .execute(&mut *tx)
                .await?;
        }
        let row: Value = sqlx::query_scalar(
            "SELECT to_jsonb(w) FROM api_cloud_workspaces w WHERE workspace_id=$1",
        )
        .bind(id)
        .fetch_one(&mut *tx)
        .await?;
        Self::reconcile_abandoned_allocation(&mut tx, &row, generation, now).await?;
        tx.commit().await?;
        Ok(crate::store::Workspace { row }.public())
    }

    pub async fn gateway_ticket(
        &self,
        id: &str,
        access: &crate::access::Access,
        protocol: &str,
        rpc_protocol: Option<&str>,
        device: &str,
        now: i64,
    ) -> Result<Value, ApiError> {
        use zuse_protocol::cloud_gateway::{PENDING_PROTOCOL, PROTOCOL, RUST_PROTOCOL};
        if ![PROTOCOL, PENDING_PROTOCOL, RUST_PROTOCOL].contains(&protocol)
            || device.is_empty()
            || device.len() > 512
        {
            return Err(ApiError::bad_request("invalid_workspace_gateway_protocol"));
        }
        self.scope_access(id, Some(&access.actor.account_id), false)
            .await?;
        let mut tx = self.pool.begin().await?;
        let (row, generation, epoch) = locked_workspace(&mut tx, id).await?;
        if row["account_id"] != access.owner_id {
            return Err(ApiError::not_found("cloud_workspace_not_found"));
        }
        let rpc_protocol = negotiate_rpc(&row["request_config"], rpc_protocol)?;
        if (rpc_protocol == zuse_protocol::cloud_gateway::RUST_RPC) != (protocol == RUST_PROTOCOL) {
            return Err(ApiError::conflict("workspace_rpc_protocol_mismatch"));
        }
        let permission = access.workspace_permission(&access.owner_id, &row["request_config"])?;
        if blocked(&row) || row["desired_state"] != "ready" || row["state"] == "failed" {
            return Err(ApiError::conflict("cloud_workspace_unavailable"));
        }
        let credential=self.signer.client(json!({"sub":access.owner_id,"workspaceId":id,"actorId":access.actor.account_id,"deviceId":device,"permission":permission,"protocol":protocol,"rpcProtocol":rpc_protocol,"generation":generation,"gatewayEpoch":epoch}),id,now)?;
        tx.commit().await?;
        Ok(
            json!({"workspaceId":id,"wsUrl":self.signer.gateway_url(id),"workspaceScope":access.scope,"protocol":protocol,"rpcProtocol":rpc_protocol,"role":"client","generation":generation,"gatewayEpoch":epoch,"credential":credential,"expiresAt":now+crate::tickets::CLIENT_TTL_MS}),
        )
    }

    #[tracing::instrument(name="cloud_runtime_request", skip_all, fields(workspace=%id, operation="gateway_authorize"))]
    pub async fn gateway_authorize(
        &self,
        id: &str,
        protocol: &str,
        credential: &str,
        now: i64,
    ) -> Result<crate::gateway::Grant, ApiError> {
        self.gateway_authorize_with_storage(id, protocol, credential, now, None)
            .await
    }
    pub async fn gateway_authorize_with_storage(
        &self,
        id: &str,
        protocol: &str,
        credential: &str,
        now: i64,
        storage: Option<zuse_protocol::cloud::CloudStorageEnrollment>,
    ) -> Result<crate::gateway::Grant, ApiError> {
        let early: Value = sqlx::query_scalar(
            "SELECT to_jsonb(w) FROM api_cloud_workspaces w WHERE workspace_id=$1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| ApiError::not_found("cloud_workspace_not_found"))?;
        let signed = self
            .signer
            .verify_gateway(credential, id, protocol, now)
            .ok();
        if signed.is_none() {
            // Gateway admission also supports retained legacy runtime bearers.
            // The command-mailbox authenticator intentionally requires native
            // RPC and must not reject that older transport before negotiation.
            if ![
                GATEWAY_PROTOCOL,
                zuse_protocol::cloud_gateway::LEGACY_PROTOCOL,
                zuse_protocol::cloud_gateway::RUST_PROTOCOL,
            ]
            .contains(&protocol)
                || !enrollment_matches(&early)
                || blocked(&early)
                || early["desired_state"] != "ready"
                || early["runtime_credential_hash"]
                    .as_str()
                    .is_none_or(|h| !equal(h, &hash(credential)))
                || early["request_config"]["runtimeCredentialExpiresAtMs"]
                    .as_i64()
                    .unwrap_or(0)
                    <= now
            {
                log_fence("gateway", id, &early, credential, now);
                return Err(ApiError::unauthorized("workspace_runtime_fenced"));
            }
        }
        let signed_actor = signed.and_then(|claims| claims["actorId"].as_str().map(str::to_owned));
        let scope_access = self
            .scope_access(id, signed_actor.as_deref(), false)
            .await?;
        let mut tx = self.pool.begin().await?;
        let (mut row, generation, epoch) = locked_workspace(&mut tx, id).await?;
        if blocked(&row) || row["desired_state"] != "ready" {
            return Err(ApiError::unauthorized("workspace_gateway_rejected"));
        }
        let owner = required(&row, "account_id")?.to_owned();
        // Preserve raw runtime bearers for retained guests; browsers use signed tickets.
        let (claims, signed) = match self.signer.verify_gateway(credential, id, protocol, now) {
            Ok(claims) => (claims, true),
            Err(_)
                if [
                    GATEWAY_PROTOCOL,
                    zuse_protocol::cloud_gateway::LEGACY_PROTOCOL,
                    zuse_protocol::cloud_gateway::RUST_PROTOCOL,
                ]
                .contains(&protocol)
                    && enrollment_matches(&row)
                    && row["runtime_credential_hash"]
                        .as_str()
                        .is_some_and(|h| equal(h, &hash(credential)))
                    && row["request_config"]["runtimeCredentialExpiresAtMs"]
                        .as_i64()
                        .is_some_and(|e| e > now) =>
            {
                (
                    json!({"role":"runtime","sub":owner,"generation":generation,"gatewayEpoch":epoch,"exp":row["request_config"]["runtimeCredentialExpiresAtMs"].as_i64().unwrap_or(0)/1000}),
                    false,
                )
            }
            Err(error) => return Err(error),
        };
        let expected_rpc = negotiate_rpc(
            &row["request_config"],
            row["request_config"]["runtimeRpcProtocol"].as_str(),
        )?
        .to_owned();
        if (expected_rpc == zuse_protocol::cloud_gateway::RUST_RPC)
            != (protocol == zuse_protocol::cloud_gateway::RUST_PROTOCOL)
        {
            return Err(ApiError::conflict("workspace_rpc_protocol_mismatch"));
        }
        if signed {
            let actual_rpc = claims["rpcProtocol"]
                .as_str()
                .unwrap_or(zuse_protocol::cloud_gateway::LEGACY_RPC);
            if actual_rpc != expected_rpc {
                return Err(ApiError::conflict("workspace_rpc_protocol_mismatch"));
            }
        }
        if claims["sub"] != owner
            || claims["generation"] != generation
            || claims["gatewayEpoch"] != epoch
        {
            return Err(ApiError::unauthorized("workspace_runtime_fenced"));
        }
        if let Some((access, policy)) = &scope_access {
            if row["request_config"]["sharingPolicy"] != *policy {
                return Err(ApiError::conflict("workspace_policy_changed"));
            }
            let permission = access.workspace_permission(&owner, &row["request_config"])?;
            if claims["role"] == "client"
                && (claims["actorId"] != access.actor.account_id
                    || (claims["permission"] == "edit"
                        && permission != crate::access::Permission::Edit))
            {
                return Err(ApiError::forbidden("workspace_access_denied"));
            }
        }

        if let Some(storage) = storage {
            if claims["role"] != "runtime"
                || expected_rpc != zuse_protocol::cloud_gateway::RUST_RPC
                || !startup_binding::negotiated(&row["request_config"], &row, generation)
                || storage.phase != zuse_protocol::cloud::CloudStoragePhase::Commit
            {
                return Err(ApiError::conflict("runtime_storage_enrollment_rejected"));
            }
            row = self
                .enroll_storage_tx(&mut tx, &row, generation, epoch, storage, now)
                .await?
                .0;
        }
        if claims["role"] == "runtime"
            && expected_rpc == zuse_protocol::cloud_gateway::RUST_RPC
            && !storage_enrolled(&row)
        {
            return Err(ApiError::conflict("runtime_storage_enrollment_required"));
        }
        let grant = crate::gateway::Grant {
            workspace: id.into(),
            generation,
            epoch,
            runtime: claims["role"] == "runtime",
            protocol: protocol.into(),
            actor: claims["actorId"].as_str().unwrap_or(&owner).into(),
            permission: claims["permission"].as_str().unwrap_or("edit").into(),
            expires_at: claims["exp"].as_i64().unwrap_or(0) * 1000,
        };
        if grant.runtime && row["request_config"]["rustFastStartVersion"] == 1 {
            // The authenticated upgrade is the durable bootstrap ACK. A lost
            // upgrade response is safe: the same identity can reconnect.
            sqlx::query("UPDATE api_cloud_workspaces SET runtime_boot_token_hash=NULL,runtime_boot_token_expires_at=NULL,request_config=jsonb_set(jsonb_set(request_config,'{runtimeBootstrapReceipt,acknowledgedAtMs}',to_jsonb($2::bigint)), '{startupTimings,gatewayAuthorizedAt}',to_jsonb($2::bigint)) WHERE workspace_id=$1 AND request_config#>'{runtimeBootstrapReceipt,acknowledgedAtMs}' IS NULL")
                .bind(id).bind(now).execute(&mut *tx).await?;
        }
        tx.commit().await?;
        Ok(grant)
    }

    #[tracing::instrument(name="cloud_runtime_request", skip_all, fields(workspace=%id, operation="gateway_lease"))]
    pub async fn gateway_lease(
        &self,
        id: &str,
        token: &str,
        fence: (i64, i64),
    ) -> Result<(), ApiError> {
        self.scope_access(id, None, true).await?;
        let mut tx = self.pool.begin().await?;
        let (row, generation, epoch) = locked_workspace(&mut tx, id).await?;
        if blocked(&row) || row["desired_state"] != "ready" || (generation, epoch) != fence {
            return Err(ApiError::unauthorized("workspace_runtime_fenced"));
        }
        let leased:Option<String>=sqlx::query_scalar("INSERT INTO zuse_rust_gateway_leases (workspace_id,owner_token,expires_at) VALUES ($1,$2,(extract(epoch from clock_timestamp())*1000)::bigint+10000) ON CONFLICT (workspace_id) DO UPDATE SET owner_token=EXCLUDED.owner_token,expires_at=EXCLUDED.expires_at WHERE zuse_rust_gateway_leases.expires_at<=(extract(epoch from clock_timestamp())*1000)::bigint OR zuse_rust_gateway_leases.owner_token=$2 RETURNING owner_token")
            .bind(id).bind(token).fetch_optional(&mut *tx).await?;
        if leased.is_none() {
            return Err(ApiError::unavailable("workspace_gateway_owned_elsewhere"));
        }
        tx.commit().await?;
        Ok(())
    }
    pub async fn release_gateway(&self, id: &str, token: &str) {
        let _ = sqlx::query(
            "DELETE FROM zuse_rust_gateway_leases WHERE workspace_id=$1 AND owner_token=$2",
        )
        .bind(id)
        .bind(token)
        .execute(&self.pool)
        .await;
    }

    #[tracing::instrument(name="cloud_runtime_request", skip_all, fields(workspace=%id, operation="renew"))]
    pub async fn renew(
        &self,
        id: &str,
        credential: &str,
        request: RenewRequest,
        now: i64,
    ) -> Result<Value, ApiError> {
        if request.request_id.is_empty()
            || request.request_id.len() > 512
            || credential.is_empty()
            || credential.len() > 512
        {
            return Err(ApiError::bad_request("invalid_runtime_renewal_request"));
        }
        self.renewal_scope_access(id, credential, &request, now)
            .await?;
        let mut tx = self.pool.begin().await?;
        let (row, generation, epoch) = locked_workspace(&mut tx, id).await?;
        if !enrollment_matches(&row) || blocked(&row) || row["desired_state"] != "ready" {
            return Err(ApiError::unauthorized("workspace_runtime_revoked"));
        }
        let mut config = row["request_config"].clone();
        let proof_verified = match config["runtimeSigningPublicJwk"].as_str() {
            Some(public) => {
                let thumbprint = signing_thumbprint(public)?;
                if config["runtimeSigningKeyThumbprint"] != thumbprint {
                    return Err(ApiError::unauthorized(
                        "runtime_signing_key_binding_mismatch",
                    ));
                }
                verify_renewal_proof(
                    &request.proof,
                    public,
                    &self.signer.issuer(),
                    id,
                    &request.request_id,
                    (generation, epoch),
                    now,
                )?;
                true
            }
            None => false, // Legacy guests may renew only their still-valid current credential.
        };
        let current_hash = hash(credential);
        let next = format!(
            "workspace_runtime_{}",
            hash(&format!("{credential}:{id}:{}", request.request_id))
        );
        let next_hash = hash(&next);
        let prior = &config["runtimeCredentialRenewal"];
        let replay = prior["requestId"] == request.request_id;
        let mut expires = now + RUNTIME_TTL_MS;
        if replay {
            if prior["previousCredentialHash"]
                .as_str()
                .is_none_or(|h| !equal(h, &current_hash))
                || prior["credentialHash"]
                    .as_str()
                    .is_none_or(|h| !equal(h, &next_hash))
                || row["runtime_credential_hash"]
                    .as_str()
                    .is_none_or(|h| !equal(h, &next_hash))
                || prior["generation"] != generation
                || prior["gatewayEpoch"] != epoch
            {
                return Err(rejected());
            }
            let previous_expiry = prior["expiresAtMs"].as_i64().ok_or_else(rejected)?;
            if previous_expiry > now {
                expires = previous_expiry;
            } else if !proof_verified {
                return Err(rejected());
            }
        } else if row["runtime_credential_hash"]
            .as_str()
            .is_none_or(|h| !equal(h, &current_hash))
            || (config["runtimeCredentialExpiresAtMs"].as_i64().unwrap_or(0) <= now
                && !proof_verified)
        {
            return Err(rejected());
        }
        if !replay || prior["expiresAtMs"].as_i64().unwrap_or(0) <= now {
            config["runtimeCredentialExpiresAtMs"] = json!(expires);
            config["runtimeCredentialRenewal"] = json!({"requestId":request.request_id,"credentialHash":next_hash,"previousCredentialHash":current_hash,"expiresAtMs":expires,"generation":generation,"gatewayEpoch":epoch});
            sqlx::query("UPDATE api_cloud_workspaces SET runtime_credential_hash=$2,request_config=$3 WHERE workspace_id=$1")
                .bind(id).bind(next_hash).bind(config).execute(&mut *tx).await?;
        }
        tx.commit().await?;
        Ok(
            json!({"workspaceId":id,"requestId":request.request_id,"runtimeCredential":next,"expiresAt":expires,"generation":generation,"gatewayEpoch":epoch}),
        )
    }

    pub async fn acknowledge(
        &self,
        id: &str,
        credential: &str,
        request: BootstrapAck,
        now: i64,
    ) -> Result<(), ApiError> {
        let mut tx = self.pool.begin().await?;
        let (row, generation, epoch) = locked_workspace(&mut tx, id).await?;
        let mut config = row["request_config"].clone();
        let receipt: Receipt = serde_json::from_value(config["runtimeBootstrapReceipt"].clone())
            .map_err(|_| rejected())?;
        if receipt.workspace_id != id
            || blocked(&row)
            || row["desired_state"] == "archived"
            || row["desired_state"] == "deleted"
            || generation != request.runtime_generation
            || epoch != request.gateway_epoch
            || receipt.generation != generation
            || receipt.gateway_epoch != epoch
            || config["runtimeCredentialExpiresAtMs"].as_i64().unwrap_or(0) <= now
            || row["runtime_credential_hash"]
                .as_str()
                .is_none_or(|h| !equal(h, &hash(credential)))
        {
            return Err(ApiError::unauthorized("workspace_runtime_fenced"));
        }
        if receipt.acknowledged_at_ms.is_none() {
            config["runtimeBootstrapReceipt"]["acknowledgedAtMs"] = json!(now);
            sqlx::query("UPDATE api_cloud_workspaces SET runtime_boot_token_hash=NULL,runtime_boot_token_expires_at=NULL,request_config=$2,revision=revision+1,updated_at=GREATEST(updated_at+1,$3) WHERE workspace_id=$1")
                .bind(id).bind(config).bind(now).execute(&mut *tx).await?;
        }
        tx.commit().await?;
        Ok(())
    }
}

/// Object I/O retains the resource generation/deletion fence without blocking
/// command metadata. Exclusive lifecycle/delete paths keep their original lock.
#[derive(Clone, Copy)]
enum WorkspaceLock {
    Exclusive,
    Command,
    Artifact,
}
async fn locked_workspace(
    tx: &mut Transaction<'_, Postgres>,
    id: &str,
) -> Result<(Value, i64, i64), ApiError> {
    locked_workspace_with(tx, id, WorkspaceLock::Exclusive).await
}
async fn locked_workspace_with(
    tx: &mut Transaction<'_, Postgres>,
    id: &str,
    mode: WorkspaceLock,
) -> Result<(Value, i64, i64), ApiError> {
    // Materialize the ownership row first: the dependent workspace CTE cannot
    // lock a workspace before its Rust ownership/generation fence is held.
    // Keep both original lock modes while paying one network round trip.
    let statement = match mode {
        WorkspaceLock::Exclusive => {
            "WITH owner AS MATERIALIZED (SELECT generation FROM zuse_rust_control_owners WHERE resource_kind='workspace' AND resource_id=$1 AND backend='rust' FOR SHARE), workspace AS MATERIALIZED (SELECT to_jsonb(w) AS row FROM api_cloud_workspaces w WHERE workspace_id=$1 AND EXISTS(SELECT 1 FROM owner) FOR UPDATE) SELECT (SELECT generation FROM owner),(SELECT row FROM workspace)"
        }
        WorkspaceLock::Command => {
            "WITH owner AS MATERIALIZED (SELECT generation FROM zuse_rust_control_owners WHERE resource_kind='workspace' AND resource_id=$1 AND backend='rust' FOR SHARE), workspace AS MATERIALIZED (SELECT to_jsonb(w) AS row FROM api_cloud_workspaces w WHERE workspace_id=$1 AND EXISTS(SELECT 1 FROM owner) FOR NO KEY UPDATE) SELECT (SELECT generation FROM owner),(SELECT row FROM workspace)"
        }
        WorkspaceLock::Artifact => {
            "WITH owner AS MATERIALIZED (SELECT generation FROM zuse_rust_control_owners WHERE resource_kind='workspace' AND resource_id=$1 AND backend='rust' FOR SHARE), workspace AS MATERIALIZED (SELECT to_jsonb(w) AS row FROM api_cloud_workspaces w WHERE workspace_id=$1 AND EXISTS(SELECT 1 FROM owner) FOR KEY SHARE) SELECT (SELECT generation FROM owner),(SELECT row FROM workspace)"
        }
    };
    let (generation, row): (Option<i64>, Option<Value>) = sqlx::query_as(statement)
        .bind(id)
        .fetch_one(&mut **tx)
        .await?;
    let owner_generation =
        generation.ok_or_else(|| ApiError::unauthorized("workspace_runtime_fenced"))?;
    let row = row.ok_or_else(rejected)?;
    let generation = row["request_config"]["runtimeGeneration"]
        .as_i64()
        .unwrap_or(1);
    let epoch = row["request_config"]["gatewayEpoch"]
        .as_i64()
        .unwrap_or(generation);
    if owner_generation != generation || generation < 1 || epoch < 1 {
        tracing::warn!(
            workspace = %id,
            site = "locked_workspace",
            reason = "generation_mismatch",
            owner_generation,
            generation,
            epoch,
            "Cloud runtime fenced"
        );
        return Err(ApiError::unauthorized("workspace_runtime_fenced"));
    }
    Ok((row, generation, epoch))
}
/// The first runtime-authentication check a row fails, for logs only: a
/// stable code, never a credential, hash or config value.
pub(super) fn fence_reason(row: &Value, credential: &str, now: i64) -> &'static str {
    if blocked(row) {
        "workspace_blocked"
    } else if row["desired_state"] != "ready" {
        "desired_state_not_ready"
    } else if row["runtime_credential_hash"].is_null() {
        "credential_missing"
    } else if row["runtime_credential_hash"]
        .as_str()
        .is_none_or(|h| !equal(h, &hash(credential)))
    {
        "credential_mismatch"
    } else if row["request_config"]["runtimeCredentialExpiresAtMs"]
        .as_i64()
        .unwrap_or(0)
        <= now
    {
        "credential_expired"
    } else if !enrollment_matches(row) {
        "bootstrap_receipt_mismatch"
    } else {
        "other_fence"
    }
}
/// Record why a runtime was fenced so intermittent startup failures are
/// diagnosable from one log line.
pub(super) fn log_fence(site: &'static str, id: &str, row: &Value, credential: &str, now: i64) {
    tracing::warn!(
        workspace = %id,
        site,
        reason = fence_reason(row, credential, now),
        generation = row["request_config"]["runtimeGeneration"].as_i64().unwrap_or(0),
        state = row["state"].as_str().unwrap_or(""),
        runtime_state = row["runtime_state"].as_str().unwrap_or(""),
        "Cloud runtime fenced"
    );
}
fn required<'a>(row: &'a Value, key: &str) -> Result<&'a str, ApiError> {
    row[key].as_str().ok_or_else(rejected)
}
fn blocked(row: &Value) -> bool {
    matches!(
        row["state"].as_str(),
        Some("archived" | "archiving" | "deleting" | "deleted")
    ) || row["desired_state"] == "deleted"
        || row["request_config"]["cloudMailboxLifecyclePending"]["action"] == "delete"
        || row["request_config"]["cloudMailboxLifecycleDelivered"]["action"] == "delete"
}
fn rejected() -> ApiError {
    ApiError::unauthorized("workspace_bootstrap_rejected")
}
fn hash(value: &str) -> String {
    Sha256::digest(value.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
fn equal(a: &str, b: &str) -> bool {
    bool::from(a.as_bytes().ct_eq(b.as_bytes()))
}
fn auth_mode(value: &Value) -> &str {
    match value.as_str() {
        Some("broker-v1") => "broker-v1",
        Some("snapshot-native") => "snapshot-native",
        _ => "legacy-image",
    }
}

fn negotiate_rpc<'a>(config: &'a Value, requested: Option<&str>) -> Result<&'a str, ApiError> {
    use zuse_protocol::cloud_gateway::{LEGACY_RPC, RUST_RPC};
    let expected = config["runtimeRpcProtocol"].as_str().unwrap_or(LEGACY_RPC);
    if ![LEGACY_RPC, RUST_RPC].contains(&expected) || requested.unwrap_or(LEGACY_RPC) != expected {
        return Err(ApiError::conflict("workspace_rpc_protocol_mismatch"));
    }
    Ok(expected)
}

fn enrollment_matches(row: &Value) -> bool {
    let config = &row["request_config"];
    if config["runtimeRpcProtocol"] != zuse_protocol::cloud_gateway::RUST_RPC {
        return true;
    }
    let generation = config["runtimeGeneration"].as_i64().unwrap_or(1);
    let epoch = config["gatewayEpoch"].as_i64().unwrap_or(generation);
    let receipt = &config["runtimeBootstrapReceipt"];
    receipt["workspaceId"] == row["workspace_id"]
        && receipt["generation"].as_i64() == Some(generation)
        && receipt["gatewayEpoch"].as_i64() == Some(epoch)
}

fn receipt_fast_start(config: &Value) -> bool {
    config["runtimeBootstrapReceipt"]["capabilities"]
        .as_array()
        .is_some_and(|caps| caps.iter().any(|cap| cap == "rust-fast-start-v1"))
}
fn storage_enrolled(row: &Value) -> bool {
    let config = &row["request_config"];
    let storage = &config["rustStorageEnrollment"];
    enrollment_matches(row)
        && storage["phase"] == "committed"
        && storage["generation"].as_i64() == Some(config["runtimeGeneration"].as_i64().unwrap_or(1))
        && storage["sourceSandboxId"].as_str().is_some()
        && storage["sourceSandboxId"] == row["provider_sandbox_id"]
        && storage["storageIncarnationId"].as_str().is_some()
        && storage["storageIncarnationId"] == config["rustRuntimeStorage"]["storageIncarnationId"]
        && storage["credentialKeyThumbprint"].as_str().is_some()
        && storage["credentialKeyThumbprint"] == config["runtimeCredentialKeyThumbprint"]
        && storage["signingKeyThumbprint"].as_str().is_some()
        && storage["signingKeyThumbprint"] == config["runtimeSigningKeyThumbprint"]
}

fn native_commands_supported(row: &Value) -> bool {
    let config = &row["request_config"];
    let capabilities =
        if storage_enrolled(row) && config["rustStorageEnrollment"]["capabilities"].is_array() {
            &config["rustStorageEnrollment"]["capabilities"]
        } else if task_commands::command_mode(row) {
            return false;
        } else {
            &config["runtimeBootstrapReceipt"]["capabilities"]
        };
    enrollment_matches(row)
        && capabilities.as_array().is_some_and(|caps| {
            caps.iter().any(|cap| cap == "rust-session-commands-v1")
                && (!task_commands::command_mode(row)
                    || caps
                        .iter()
                        .any(|cap| cap == zuse_protocol::cloud_task_commands::CAPABILITY))
        })
}
/// Whether the guest runtime on this computer advertised `capability` at its
/// last bootstrap, or in its last committed runtime update attestation (which
/// replaces the stored set). A sleeping computer is judged by that record. A
/// computer that never stored any capabilities is treated as an old runtime,
/// since a guest that predates a capability never advertised it either.
pub(crate) fn runtime_capable(row: &Value, capability: &str) -> bool {
    // Only the bootstrap receipt: it is replaced on every bootstrap, while a
    // storage enrollment can outlive a replaced guest.
    row["request_config"]["runtimeBootstrapReceipt"]["capabilities"]
        .as_array()
        .is_some_and(|caps| caps.iter().any(|cap| cap == capability))
}
fn native_commands_accepted(row: &Value) -> bool {
    native_commands_supported(row)
        || (!task_commands::command_mode(row)
            && row["request_config"]["rustSessionCommandVersion"] == 1)
}

// Admission copies this version only from the verified managed image. A guest
// must also have advertised the compiled protocol in its bound bootstrap receipt.
fn recreation_bootstrap_supported(row: &Value, receipt: &Receipt) -> bool {
    let config = &row["request_config"];
    config["rustRecreationVersion"] == 1
        && config["providerAuthMode"] == "broker-v1"
        && row["managed_release_id"]
            .as_str()
            .is_some_and(|id| !id.is_empty())
        && config["managedReleaseId"] == row["managed_release_id"]
        && receipt.capabilities.as_ref().is_some_and(|caps| {
            caps.iter()
                .any(|c| c == zuse_protocol::cloud_lifecycle::RECREATION_CAPABILITY)
        })
}
#[cfg(test)]
#[path = "runtime/recreation_bootstrap_tests.rs"]
mod recreation_bootstrap_tests;

#[cfg(test)]
mod lock_tests;
impl RuntimeControl {
    /// Derive the plugin tenant only after verifying enrollment and runtime fencing.
    pub(crate) async fn plugin_identity(
        &self,
        workspace: &str,
        credential: &str,
        at: i64,
    ) -> Result<crate::plugins::PluginIdentity, ApiError> {
        let mut tx = self.pool.begin().await?;
        let (row, _, _) = locked_workspace(&mut tx, workspace).await?;
        mailbox::runtime_auth(&row, credential, at)?;
        let owner = required(&row, "account_id")?.to_owned();
        let tenant = if owner.starts_with("organization:") {
            owner.clone()
        } else {
            format!("personal:{owner}")
        };
        tx.commit().await?;
        Ok(crate::plugins::PluginIdentity {
            tenant,
            subject: owner,
        })
    }
}
