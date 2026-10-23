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
    PgPool, Postgres, Transaction,
    postgres::{PgConnectOptions, PgPoolOptions},
};
use std::{env, str::FromStr, sync::Arc, time::Duration};
use subtle::ConstantTimeEq;

pub mod allocation;
pub mod creation;
pub mod grants;
pub mod mailbox;

pub struct RuntimeConfig {
    master: MasterKey,
    signer: TicketSigner,
    idle_ms: i64,
    creation_accounts: Vec<String>,
    legacy_auth_url: Option<String>,
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
        Ok(Some(Self {
            legacy_auth_url: env::var("ZUSE_CLOUD_LEGACY_AUTH_URL").ok(),
            creation_accounts: env::var("ZUSE_CLOUD_CREATE_ACCOUNTS")
                .unwrap_or_default()
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .collect(),
            idle_ms,
            master: MasterKey::parse(&encryption)
                .map_err(|_| anyhow::anyhow!("Invalid cloud data encryption key"))?,
            signer: TicketSigner::from_jwk(&mint, &issuer)
                .map_err(|_| anyhow::anyhow!("Invalid runtime ticket signing configuration"))?,
        }))
    }
}

pub struct RuntimeControl {
    pool: PgPool,
    master: MasterKey,
    signer: TicketSigner,
    idle_ms: i64,
    creation_accounts: Vec<String>,
    grant_issuer: Option<Arc<dyn grants::GrantIssuer>>,
    pub(crate) mailbox_changes: tokio::sync::broadcast::Sender<()>,
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
    pub async fn connect(url: &str, config: RuntimeConfig) -> anyhow::Result<Arc<Self>> {
        let options = PgConnectOptions::from_str(url)
            .map_err(|_| anyhow::anyhow!("Invalid cloud PostgreSQL configuration"))?
            .application_name("zuse-cloud-runtime-control");
        let pool = PgPoolOptions::new()
            .max_connections(4)
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
        control.idle_ms = config.idle_ms;
        control.creation_accounts = config.creation_accounts;
        if let Some(url) = config.legacy_auth_url {
            control.grant_issuer = Some(Arc::new(
                grants::LegacyGrantIssuer::new(&url, &control.signer.issuer())
                    .map_err(|error| anyhow::anyhow!("{}", error.code))?,
            ));
        }
        control
            .ready()
            .await
            .map_err(|_| anyhow::anyhow!("Runtime-control schema is not ready"))?;
        Ok(Arc::new(control))
    }
    pub fn from_pool(pool: PgPool, master: MasterKey, signer: TicketSigner) -> Self {
        Self {
            pool,
            master,
            signer,
            idle_ms: 10 * 60 * 1000,
            creation_accounts: vec![],
            grant_issuer: None,
            mailbox_changes: tokio::sync::broadcast::channel(128).0,
        }
    }
    pub async fn ready(&self) -> Result<(), ApiError> {
        let ready: bool = sqlx::query_scalar("SELECT to_regclass('zuse_rust_control_owners') IS NOT NULL AND to_regclass('api_cloud_workspace_launch_intents') IS NOT NULL AND to_regclass('zuse_rust_gateway_leases') IS NOT NULL AND to_regclass('zuse_rust_mailbox_commands') IS NOT NULL")
            .fetch_one(&self.pool).await?;
        if !ready {
            return Err(ApiError::unavailable("cloud_runtime_schema_not_ready"));
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
        if self.creation_enabled() {
            self.allocation_ready().await?;
            let ready:bool=sqlx::query_scalar("SELECT to_regclass('api_cloud_projects') IS NOT NULL AND to_regclass('api_cloud_project_builds') IS NOT NULL AND to_regclass('api_cloud_snapshots') IS NOT NULL AND to_regclass('api_cloud_billing_periods') IS NOT NULL AND to_regclass('api_cloud_billing_ledger') IS NOT NULL AND to_regclass('api_cloud_billing_reservations') IS NOT NULL").fetch_one(&self.pool).await?;
            if !ready {
                return Err(ApiError::unavailable("cloud_creation_schema_not_ready"));
            }
        }
        Ok(())
    }

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
        if (owner.starts_with("organization:")
            || config["initialGithubContext"]["slackMessageId"].is_string())
            && !request
                .capabilities
                .as_ref()
                .is_some_and(|caps| caps.iter().any(|c| c == "github-execution-v1"))
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
            let mut timings = config
                .get("startupTimings")
                .filter(|v| v.is_object())
                .cloned()
                .unwrap_or_else(|| json!({}));
            timings["enrolledAt"] = json!(now);
            timings["runtimeReadyAt"] = json!(now);
            if let Some(allocated) = timings["allocatedAt"].as_i64() {
                timings["enrollmentDurationMs"] = json!(now.saturating_sub(allocated));
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
        if let Some(intent) = intent {
            response["launchIntent"] = intent;
        }
        if rpc_protocol == zuse_protocol::cloud_gateway::RUST_RPC {
            response["commandMailboxEnabled"] = json!(config["rustCommandMailboxEnabled"] == true);
            if let Some(plan) = config.get("rustRepositoryCheckout") {
                response["rustRepositoryCheckout"] = plan.clone();
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
            response["runtimeStorage"] = serde_json::to_value(storage)
                .map_err(|_| ApiError::conflict("runtime_storage_enrollment_required"))?;
        }
        if let Some(context) = config.get("initialGithubContext") {
            response["initialGithubContext"] = context.clone();
        }
        tx.commit().await?;
        Ok(response)
    }

    pub async fn enroll_storage(
        &self,
        id: &str,
        credential: &str,
        request: zuse_protocol::cloud::CloudStorageEnrollment,
        now: i64,
    ) -> Result<Value, ApiError> {
        use zuse_protocol::cloud::CloudStoragePhase;
        if request.capabilities.as_ref().is_some_and(|caps| {
            caps.len() > 2
                || caps
                    .iter()
                    .any(|cap| !matches!(cap.as_str(), "rust-rpc-v1" | "rust-session-commands-v1"))
        }) {
            return Err(ApiError::bad_request("invalid_runtime_capabilities"));
        }
        let mut tx = self.pool.begin().await?;
        let (row, generation, epoch) = locked_workspace(&mut tx, id).await?;
        let mut config = row["request_config"].clone();
        if !enrollment_matches(&row)
            || blocked(&row)
            || row["desired_state"] != "ready"
            || request.runtime_generation != generation
            || request.gateway_epoch != epoch
            || row["runtime_credential_hash"]
                .as_str()
                .is_none_or(|h| !equal(h, &hash(credential)))
            || config["runtimeCredentialExpiresAtMs"].as_i64().unwrap_or(0) <= now
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
                {
                    return Err(ApiError::conflict("runtime_storage_recovery_required"));
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
            }
        }
        if config.get("rustStorageEnrollment") != Some(&record) {
            config["rustStorageEnrollment"] = record;
            sqlx::query("UPDATE api_cloud_workspaces SET request_config=$2,revision=revision+1,updated_at=GREATEST(updated_at+1,$3) WHERE workspace_id=$1").bind(id).bind(config).bind(now).execute(&mut *tx).await?;
        }
        tx.commit().await?;
        Ok(json!({"enrollmentId":enrollment}))
    }

    pub async fn ready_launch(
        &self,
        id: &str,
        credential: &str,
        request: zuse_protocol::cloud::CloudRuntimeReady,
        now: i64,
    ) -> Result<Value, ApiError> {
        if request.phase != "agent-started" || request.session_head_version.is_none_or(|v| v < 0) {
            return Err(ApiError::bad_request("invalid_runtime_ready_request"));
        }
        let command = request
            .launch_command_id
            .filter(|s| !s.is_empty() && s.len() <= 512)
            .ok_or_else(|| ApiError::bad_request("invalid_runtime_ready_request"))?;
        let head = request.session_head_version.unwrap_or(0);
        let mut tx = self.pool.begin().await?;
        let (row, _, _) = locked_workspace(&mut tx, id).await?;
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

        let intent:Option<String>=sqlx::query_scalar("SELECT command_id FROM api_cloud_workspace_launch_intents WHERE workspace_id=$1 FOR UPDATE").bind(id).fetch_optional(&mut *tx).await?;
        if intent.as_deref() != Some(&command)
            && !(intent.is_none() && config["rustCompletedLaunchCommandId"] == command)
        {
            return Err(ApiError::conflict("launch_intent_receipt_rejected"));
        }
        let previous = config["sessionHeadVersion"].as_i64().unwrap_or(0);
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
        if blocked(&row) || row["desired_state"] == "archived" || row["state"] == "failed" {
            return Err(ApiError::conflict("cloud_workspace_unavailable"));
        }
        // Organization execution requires the runtime access callback, ported separately.
        if access.membership.is_some() {
            return Err(ApiError::conflict("workspace_runtime_update_required"));
        }
        let credential=self.signer.client(json!({"sub":access.owner_id,"workspaceId":id,"actorId":access.actor.account_id,"deviceId":device,"permission":permission,"protocol":protocol,"rpcProtocol":rpc_protocol,"generation":generation,"gatewayEpoch":epoch}),id,now)?;
        tx.commit().await?;
        Ok(
            json!({"workspaceId":id,"wsUrl":self.signer.gateway_url(id),"workspaceScope":access.scope,"protocol":protocol,"rpcProtocol":rpc_protocol,"role":"client","generation":generation,"gatewayEpoch":epoch,"credential":credential,"expiresAt":now+crate::tickets::CLIENT_TTL_MS}),
        )
    }

    pub async fn gateway_authorize(
        &self,
        id: &str,
        protocol: &str,
        credential: &str,
        now: i64,
    ) -> Result<crate::gateway::Grant, ApiError> {
        let mut tx = self.pool.begin().await?;
        let (row, generation, epoch) = locked_workspace(&mut tx, id).await?;
        if blocked(&row) || row["desired_state"] == "archived" {
            return Err(ApiError::unauthorized("workspace_gateway_rejected"));
        }
        let owner = required(&row, "account_id")?;
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
        )?;
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
        if owner.starts_with("organization:") {
            return Err(ApiError::conflict("workspace_runtime_update_required"));
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
            actor: claims["actorId"].as_str().unwrap_or(owner).into(),
            permission: claims["permission"].as_str().unwrap_or("edit").into(),
            expires_at: claims["exp"].as_i64().unwrap_or(0) * 1000,
        };
        tx.commit().await?;
        Ok(grant)
    }

    pub async fn gateway_lease(
        &self,
        id: &str,
        token: &str,
        fence: (i64, i64),
    ) -> Result<(), ApiError> {
        let mut tx = self.pool.begin().await?;
        let (row, generation, epoch) = locked_workspace(&mut tx, id).await?;
        if blocked(&row) || row["desired_state"] == "archived" || (generation, epoch) != fence {
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
        let mut tx = self.pool.begin().await?;
        let (row, generation, epoch) = locked_workspace(&mut tx, id).await?;
        if !enrollment_matches(&row) || blocked(&row) || row["desired_state"] == "archived" {
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

async fn locked_workspace(
    tx: &mut Transaction<'_, Postgres>,
    id: &str,
) -> Result<(Value, i64, i64), ApiError> {
    let generation:Option<i64>=sqlx::query_scalar("SELECT generation FROM zuse_rust_control_owners WHERE resource_kind='workspace' AND resource_id=$1 AND backend='rust' FOR SHARE")
        .bind(id).fetch_optional(&mut **tx).await?;
    let owner_generation =
        generation.ok_or_else(|| ApiError::unauthorized("workspace_runtime_fenced"))?;
    let row: Option<Value> = sqlx::query_scalar(
        "SELECT to_jsonb(w) FROM api_cloud_workspaces w WHERE workspace_id=$1 FOR UPDATE",
    )
    .bind(id)
    .fetch_optional(&mut **tx)
    .await?;
    let row = row.ok_or_else(rejected)?;
    let generation = row["request_config"]["runtimeGeneration"]
        .as_i64()
        .unwrap_or(1);
    let epoch = row["request_config"]["gatewayEpoch"]
        .as_i64()
        .unwrap_or(generation);
    if owner_generation != generation || generation < 1 || epoch < 1 {
        return Err(ApiError::unauthorized("workspace_runtime_fenced"));
    }
    Ok((row, generation, epoch))
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
        } else {
            &config["runtimeBootstrapReceipt"]["capabilities"]
        };
    enrollment_matches(row)
        && capabilities
            .as_array()
            .is_some_and(|caps| caps.iter().any(|cap| cap == "rust-session-commands-v1"))
}
fn native_commands_accepted(row: &Value) -> bool {
    native_commands_supported(row) || row["request_config"]["rustSessionCommandVersion"] == 1
}
