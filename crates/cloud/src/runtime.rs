//! Runtime bootstrap preserves original storage identity and durable enrollment receipts.
//! Only explicitly Rust-owned workspaces admit writes; no provider operations run here.
use crate::{
    crypto::{MasterKey, credential_thumbprint, seal_runtime_secret},
    error::ApiError,
    tickets::{GATEWAY_PROTOCOL, RUNTIME_TTL_MS, TicketSigner, signing_thumbprint},
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

pub struct RuntimeConfig {
    master: MasterKey,
    signer: TicketSigner,
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
        Ok(Some(Self {
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
}

pub use zuse_protocol::cloud::{
    CloudRuntimeBootstrapAck as BootstrapAck, CloudRuntimeBootstrapRequest as BootstrapRequest,
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
        let control = Self::from_pool(pool, config.master, config.signer);
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
        }
    }
    pub async fn ready(&self) -> Result<(), ApiError> {
        let ready: bool = sqlx::query_scalar("SELECT to_regclass('zuse_rust_control_owners') IS NOT NULL AND to_regclass('api_cloud_workspace_launch_intents') IS NOT NULL")
            .fetch_one(&self.pool).await?;
        if !ready {
            return Err(ApiError::unavailable("cloud_runtime_schema_not_ready"));
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
        let gateway_credential = self.signer.runtime(
            owner,
            id,
            generation,
            epoch,
            receipt.runtime_credential_expires_at_ms,
        )?;
        let scope = owner
            .strip_prefix("organization:")
            .map(|org| json!({"kind":"organization","organizationId":org}))
            .unwrap_or_else(|| json!({"kind":"personal"}));
        let mut response = json!({"workspaceId":id,"zuseAccountId":owner,"workspaceScope":scope,"providerSandboxId":row["provider_sandbox_id"],
            "runtimeCredential":runtime_credential,"runtimeGatewayCredential":gateway_credential,"cloudCredentials":[],
            "gatewayUrl":self.signer.gateway_url(id),"gatewayProtocol":GATEWAY_PROTOCOL,"chatId":row["chat_id"],"initialSessionId":row["initial_session_id"],
            "codexAuthMode":auth_mode(&config["codexAuthMode"]),"providerAuthMode":auth_mode(&config["providerAuthMode"]),
            "runtimeGeneration":generation,"gatewayEpoch":epoch,"runtimeCredentialExpiresAt":receipt.runtime_credential_expires_at_ms,
            "sealedTranscriptKey":receipt.sealed_transcript_key});
        if let Some(intent) = intent {
            response["launchIntent"] = intent;
        }
        if let Some(context) = config.get("initialGithubContext") {
            response["initialGithubContext"] = context.clone();
        }
        tx.commit().await?;
        Ok(response)
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
            || !equal(&receipt.runtime_credential_hash, &hash(credential))
            || receipt.runtime_credential_expires_at_ms <= now
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
    )
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
