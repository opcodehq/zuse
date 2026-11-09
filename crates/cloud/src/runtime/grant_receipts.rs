//! Persist a request before renewal, and replay only its recipient-sealed result.
use super::*;
use zuse_protocol::cloud_auth::{ProviderGrantRequest, SealedProviderGrant};
pub(super) struct GrantReceipt {
    workspace: String,
    generation: i64,
    thumb: String,
    request: String,
    digest: String,
    lease: String,
    owner: String,
    credential_owner: String,
    connection: String,
    epoch: i64,
    source_version: i64,
    pub expires_at: i64,
    pub cached: Option<SealedProviderGrant>,
}
impl GrantReceipt {
    fn aad(&self) -> String {
        format!(
            "runtime-grant-receipt:{}:{}:{}:{}:{}:{}:{}:{}:{}:{}",
            self.workspace,
            self.generation,
            self.thumb,
            self.request,
            self.digest,
            self.owner,
            self.credential_owner,
            self.connection,
            self.epoch,
            self.source_version
        )
    }
}
impl RuntimeControl {
    pub(super) async fn begin_grant_receipt(
        &self,
        id: &str,
        owner: &str,
        credential_owner: &str,
        request: &ProviderGrantRequest,
        binding: &(String, String, i64),
        now: i64,
    ) -> Result<GrantReceipt, ApiError> {
        // Keep the independent autocommit statements fresh, but avoid returning
        // this connection between them (each checkout has its own health ping).
        // The connection is released before any credential/vendor work begins.
        let mut database_connection = self.pool.acquire().await?;
        let version:Option<i64>=sqlx::query_scalar("SELECT version FROM cloud_agent_connections WHERE owner_id=$1 AND connection_id=$2 AND state='connected' AND authority_epoch=$3").bind(credential_owner).bind(&binding.1).bind(binding.2).fetch_optional(&mut *database_connection).await?;
        let version =
            version.ok_or_else(|| ApiError::conflict("cloud_provider_grant_fence_mismatch"))?;
        let mut receipt = GrantReceipt {
            workspace: id.into(),
            generation: request.runtime_generation,
            thumb: binding.0.clone(),
            request: request.request_id.clone(),
            digest: hash(
                &serde_json::to_string(request)
                    .map_err(|_| ApiError::bad_request("invalid_provider_grant_request"))?,
            ),
            lease: uuid::Uuid::new_v4().to_string(),
            owner: owner.into(),
            credential_owner: credential_owner.into(),
            connection: binding.1.clone(),
            epoch: binding.2,
            source_version: version,
            expires_at: now + 15 * 60_000,
            cached: None,
        };
        let inserted=sqlx::query("INSERT INTO zuse_rust_provider_grant_receipts(workspace_id,runtime_generation,recipient_thumbprint,request_id,request_digest,owner_id,credential_owner_id,connection_id,authority_epoch,source_version,phase,lease_id,lease_expires_at,expires_at,created_at,updated_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,'processing',$11,$12,$13,$14,$14) ON CONFLICT(workspace_id,runtime_generation,recipient_thumbprint,request_id) DO NOTHING").bind(id).bind(request.runtime_generation).bind(&binding.0).bind(&request.request_id).bind(&receipt.digest).bind(owner).bind(credential_owner).bind(&binding.1).bind(binding.2).bind(version).bind(&receipt.lease).bind(now+60_000).bind(receipt.expires_at).bind(now).execute(&mut *database_connection).await?.rows_affected();
        if inserted == 1 {
            return Ok(receipt);
        }
        let row:(String,String,String,String,i64,String,Option<String>,i64,i64,i64)=sqlx::query_as("SELECT request_digest,owner_id,credential_owner_id,connection_id,authority_epoch,phase,grant_ciphertext,expires_at,lease_expires_at,source_version FROM zuse_rust_provider_grant_receipts WHERE workspace_id=$1 AND runtime_generation=$2 AND recipient_thumbprint=$3 AND request_id=$4").bind(id).bind(request.runtime_generation).bind(&binding.0).bind(&request.request_id).fetch_one(&mut *database_connection).await?;
        let (
            digest,
            record_owner,
            record_credential_owner,
            connection,
            epoch,
            phase,
            ciphertext,
            expiry,
            lease_until,
            source_version,
        ) = row;
        receipt.source_version = source_version;
        if digest != receipt.digest
            || record_owner != owner
            || record_credential_owner != credential_owner
            || connection != binding.1
            || epoch != binding.2
        {
            return Err(ApiError::conflict(
                "cloud_provider_grant_idempotency_conflict",
            ));
        }
        if phase == "ready" && expiry > now + 60_000 {
            let bytes = self
                .master
                .open_api(
                    &receipt.aad(),
                    &ciphertext.ok_or_else(|| {
                        ApiError::conflict("cloud_provider_grant_receipt_invalid")
                    })?,
                )
                .map_err(|_| ApiError::conflict("cloud_provider_grant_receipt_invalid"))?;
            receipt.cached = Some(
                serde_json::from_slice(&bytes)
                    .map_err(|_| ApiError::conflict("cloud_provider_grant_receipt_invalid"))?,
            );
            receipt.expires_at = expiry;
            return Ok(receipt);
        }
        if phase == "processing" && lease_until > now {
            return Err(ApiError::unavailable("cloud_provider_grant_pending"));
        }
        // Never turn a lost response or crash into a second force-refresh dispatch.
        sqlx::query("UPDATE zuse_rust_provider_grant_receipts SET phase='uncertain',grant_ciphertext=NULL,updated_at=$5 WHERE workspace_id=$1 AND runtime_generation=$2 AND recipient_thumbprint=$3 AND request_id=$4 AND phase<>'uncertain'").bind(id).bind(request.runtime_generation).bind(&binding.0).bind(&request.request_id).bind(now).execute(&mut *database_connection).await?;
        Err(ApiError::conflict("cloud_provider_grant_outcome_uncertain"))
    }
    pub(super) async fn finish_grant_receipt(
        &self,
        receipt: &GrantReceipt,
        grant: &SealedProviderGrant,
        now: i64,
    ) -> Result<(), ApiError> {
        let encoded = zeroize::Zeroizing::new(
            serde_json::to_string(grant)
                .map_err(|_| ApiError::unavailable("cloud_provider_grant_receipt_invalid"))?,
        );
        let ciphertext = self
            .master
            .seal_api(&receipt.aad(), &encoded)
            .map_err(|_| ApiError::unavailable("cloud_provider_grant_receipt_invalid"))?;
        let saved=sqlx::query("UPDATE zuse_rust_provider_grant_receipts SET phase='ready',grant_ciphertext=$7,expires_at=$8,updated_at=$9 WHERE workspace_id=$1 AND runtime_generation=$2 AND recipient_thumbprint=$3 AND request_id=$4 AND request_digest=$5 AND lease_id=$6 AND phase='processing' AND lease_expires_at>$9").bind(&receipt.workspace).bind(receipt.generation).bind(&receipt.thumb).bind(&receipt.request).bind(&receipt.digest).bind(&receipt.lease).bind(ciphertext).bind(receipt.expires_at).bind(now).execute(&self.pool).await?.rows_affected();
        if saved != 1 {
            return Err(ApiError::conflict("cloud_provider_grant_outcome_uncertain"));
        }
        Ok(())
    }
    pub async fn cleanup_grant_receipts(&self, now: i64) -> Result<(), ApiError> {
        cleanup_grant_receipts_on(&self.pool, now).await
    }
}

/// Bounded housekeeping. It never decides the outcome of a current request:
/// `begin_grant_receipt` independently rejects expired or uncertain receipts.
pub(super) async fn cleanup_grant_receipts_on(pool: &PgPool, now: i64) -> Result<(), ApiError> {
    {
        // Reuse one connection; preserve statement order and autocommit behavior.
        let mut connection = pool.acquire().await?;
        // Keep current generation request tombstones. Erasing them while bearer is valid
        // would let a delayed Unauthorized request perform another vendor refresh.
        sqlx::query("UPDATE zuse_rust_provider_grant_receipts SET phase='uncertain',grant_ciphertext=NULL,updated_at=$1 WHERE (workspace_id,runtime_generation,recipient_thumbprint,request_id) IN (SELECT workspace_id,runtime_generation,recipient_thumbprint,request_id FROM zuse_rust_provider_grant_receipts WHERE phase='ready' AND expires_at<=$1 ORDER BY expires_at LIMIT 32)").bind(now).execute(&mut *connection).await?;
        sqlx::query("DELETE FROM zuse_rust_provider_grant_receipts r WHERE (workspace_id,runtime_generation,recipient_thumbprint,request_id) IN (SELECT p.workspace_id,p.runtime_generation,p.recipient_thumbprint,p.request_id FROM zuse_rust_provider_grant_receipts p WHERE p.expires_at<$1 AND NOT EXISTS(SELECT 1 FROM api_cloud_workspaces w WHERE w.workspace_id=p.workspace_id AND w.request_config->'runtimeGeneration'=to_jsonb(p.runtime_generation)) ORDER BY p.expires_at LIMIT 32)").bind(now-86_400_000).execute(&mut *connection).await?;
        Ok(())
    }
}
