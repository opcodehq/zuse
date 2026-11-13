//! Encrypted prelaunch assets share the future workspace's purge namespace.
mod release;
mod workspace;
use super::*;
use crate::access::Access;
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
pub(super) use release::Holder;
pub use release::{STALE_CHUNK_CIPHERTEXT_MS, UNDELIVERED_ASSET_TTL_MS};
use zuse_protocol::cloud_assets::{self as wire, Asset, Chunk, Commit, Download};
pub const MIGRATION: &str = include_str!("../../migrations/0008_launch_assets.sql");
pub const RELEASE_MIGRATION: &str = include_str!("../../migrations/0035_launch_asset_release.sql");
fn invalid() -> ApiError {
    ApiError::bad_request("invalid_launch_asset")
}
fn failed() -> ApiError {
    ApiError::unavailable("launch_asset_unavailable")
}
/// The computer already received it, or it expired: upload it again to resend.
fn released() -> ApiError {
    ApiError::conflict("launch_asset_released")
}
fn digest(bytes: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(bytes))
}
fn context(owner: &str, workspace: &str, asset: &str, suffix: &str) -> String {
    format!("asset\n{owner}\n{workspace}\n{asset}{suffix}")
}
fn prefix(workspace: &str, asset: &str) -> String {
    format!("workspaces/{workspace}/api-assets/{asset}")
}
fn valid_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 200
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
}
impl RuntimeControl {
    pub(super) fn launch_idempotency(&self, access: &Access, key: &str) -> String {
        let context = if access.membership.is_some() {
            format!(
                "launch-assets:{}:actor:{}",
                access.owner_id, access.actor.account_id
            )
        } else {
            format!("launch-assets:{}", access.owner_id)
        };
        self.master.digest_api(&context, key)
    }
    async fn launch_stage(
        &self,
        access: &Access,
        key: &str,
        at: i64,
    ) -> Result<(Transaction<'_, Postgres>, String), ApiError> {
        if !super::creation::creation_actor_allowed(access)
            || !self.creation_accounts.contains(&access.owner_id)
        {
            return Err(ApiError::forbidden("cloud_workspace_creation_disabled"));
        }
        if key.is_empty() || key.len() > 512 {
            return Err(invalid());
        }
        self.scope_admission(access).await?;
        loop {
            let mut tx = self.pool.begin().await?;
            sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
                .bind(format!("cloud-create:{}", access.owner_id))
                .execute(&mut *tx)
                .await?;
            let ready:bool=sqlx::query_scalar("SELECT to_regclass('zuse_rust_launch_staging') IS NOT NULL AND to_regclass('zuse_rust_launch_assets') IS NOT NULL").fetch_one(&mut *tx).await?;
            if !ready {
                return Err(ApiError::unavailable("cloud_launch_asset_schema_not_ready"));
            }
            let dead: bool =
                sqlx::query_scalar("SELECT to_regclass('zuse_rust_deletions') IS NOT NULL")
                    .fetch_one(&mut *tx)
                    .await?;
            if dead {
                let deleted:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM zuse_rust_deletions WHERE account_id=$1 AND idempotency_digest=$2 AND (expires_at IS NULL OR expires_at>$3))").bind(&access.owner_id).bind(self.delete_idempotency(&access.owner_id,key)).bind(at).fetch_one(&mut *tx).await?;
                if deleted {
                    return Err(ApiError::conflict("cloud_workspace_deleted"));
                }
            }
            let entitled:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM api_entitlements WHERE account_id=$1 AND kind='cloud-workspace' AND (status IN ('active','grace') OR (status='ended' AND paid_through>$2)) AND (paid_through IS NULL OR paid_through>$2))").bind(&access.owner_id).bind(at).fetch_one(&mut *tx).await?;
            let staging_allowance = if let Some(pin) = self
                .staging_candidate
                .as_ref()
                .filter(|p| p.owner_id == access.owner_id)
            {
                pin.require_actor(&access.actor.account_id)?;
                pin.load(&mut tx, at, true).await?;
                pin.allow_staging_compute
            } else {
                false
            };
            if !entitled && !staging_allowance {
                return Err(ApiError::forbidden("cloud_entitlement_required"));
            }
            if !staging_allowance {
                crate::billing::require_capacity(&mut tx, &access.owner_id, at).await?;
            }
            let prior:Option<(String,String,Value)>=sqlx::query_as("SELECT workspace_id,state,request_config FROM api_cloud_workspaces WHERE account_id=$1 AND idempotency_key=$2").bind(&access.owner_id).bind(key).fetch_optional(&mut *tx).await?;
            if let Some((_, state, config)) = &prior {
                if access.membership.is_some()
                    && config["rustActorId"].as_str() != Some(access.actor.account_id.as_str())
                {
                    return Err(ApiError::conflict("cloud_workspace_idempotency_conflict"));
                }
                if matches!(state.as_str(), "deleting" | "deleted") {
                    return Err(ApiError::conflict("cloud_workspace_deleted"));
                }
            }
            let token = self.launch_idempotency(access, key);
            let bound = prior.is_some();
            let reserved = prior
                .map(|p| p.0)
                .unwrap_or_else(|| format!("workspace_{}", uuid::Uuid::new_v4().simple()));
            let stage: Option<(String,i64)> = sqlx::query_as("SELECT workspace_id,expires_at FROM zuse_rust_launch_staging WHERE account_id=$1 AND idempotency_digest=$2").bind(&access.owner_id).bind(&token).fetch_optional(&mut *tx).await?;
            let Some((workspace, expiry)) = stage else {
                sqlx::query("INSERT INTO zuse_rust_launch_staging(account_id,idempotency_digest,workspace_id,created_at,expires_at) VALUES($1,$2,$3,$4,$5)").bind(&access.owner_id).bind(token).bind(reserved).bind(at).bind(at+86_400_000).execute(&mut *tx).await?;
                // Publish the reservation before writing objects. A later database
                // failure must leave a durable namespace for retry and TTL cleanup.
                tx.commit().await?;
                continue;
            };
            if expiry <= at && !bound {
                return Err(ApiError::conflict("cloud_launch_upload_expired"));
            }
            return Ok((tx, workspace));
        }
    }
    fn asset_id(&self, owner: &str, workspace: &str, upload: &str) -> Result<String, ApiError> {
        if !valid_id(upload) {
            return Err(invalid());
        }
        Ok(format!(
            "asset_{}",
            &self
                .master
                .digest_api(&format!("api-asset-v1\n{owner}\n{workspace}"), upload)[..40]
        ))
    }
    pub async fn launch_asset_chunk(
        &self,
        access: &Access,
        request: Chunk,
        at: i64,
    ) -> Result<Value, ApiError> {
        if request.seq >= wire::MAX_BYTES.div_ceil(wire::CHUNK_BYTES)
            || request.bytes.len() > wire::CHUNK_BYTES.div_ceil(3) * 4
        {
            return Err(invalid());
        }
        let bytes = URL_SAFE_NO_PAD
            .decode(&request.bytes)
            .map_err(|_| invalid())?;
        if bytes.is_empty() || bytes.len() > wire::CHUNK_BYTES {
            return Err(invalid());
        }
        let (mut tx, workspace) = self
            .launch_stage(access, &request.idempotency_key, at)
            .await?;
        let asset = self.asset_id(&access.owner_id, &workspace, &request.upload_id)?;
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM zuse_rust_launch_assets WHERE workspace_id=$1",
        )
        .bind(&workspace)
        .fetch_one(&mut *tx)
        .await?;
        let exists:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM zuse_rust_launch_assets WHERE workspace_id=$1 AND asset_id=$2)").bind(&workspace).bind(&asset).fetch_one(&mut *tx).await?;
        if count >= wire::MAX_ASSETS as i64 && !exists {
            return Err(ApiError::bad_request("too_many_launch_assets"));
        }
        sqlx::query("INSERT INTO zuse_rust_launch_assets(workspace_id,asset_id,state,created_at,updated_at) VALUES($1,$2,'uploading',$3,$3) ON CONFLICT DO NOTHING").bind(&workspace).bind(&asset).bind(at).execute(&mut *tx).await?;
        let (mut chunks,state):(Value,String)=sqlx::query_as("SELECT chunks,state FROM zuse_rust_launch_assets WHERE workspace_id=$1 AND asset_id=$2 FOR UPDATE").bind(&workspace).bind(&asset).fetch_one(&mut *tx).await?;
        let seq = request.seq.to_string();
        if matches!(state.as_str(), "releasing" | "released") {
            return Err(released());
        }
        if state == "ready" && chunks.get(&seq).is_none() {
            return Err(ApiError::conflict("launch_asset_already_committed"));
        }
        let receipt = json!({"size":bytes.len(),"sha256":digest(&bytes)});
        if let Some(prior) = chunks.get(&seq) {
            if prior != &receipt {
                return Err(ApiError::conflict("launch_asset_chunk_conflict"));
            }
        }
        let objects = self.transcript_objects.as_ref().ok_or_else(failed)?;
        let suffix = format!("\nchunk\n{}", request.seq);
        let aad = context(&access.owner_id, &workspace, &asset, &suffix);
        let key = format!("{}/{}.json", prefix(&workspace, &asset), request.seq);
        if let Some(prior) = objects.get_archive(&key).await? {
            let encrypted = std::str::from_utf8(&prior).map_err(|_| failed())?;
            let plain = self
                .master
                .open_api(&aad, encrypted)
                .map_err(|_| failed())?;
            if URL_SAFE_NO_PAD.decode(&*plain).map_err(|_| failed())? != bytes {
                return Err(ApiError::conflict("launch_asset_chunk_conflict"));
            }
        } else {
            let sealed = self
                .master
                .seal_api(&aad, &URL_SAFE_NO_PAD.encode(&bytes))
                .map_err(|_| failed())?;
            objects.put_archive(&key, sealed.as_bytes()).await?;
        }
        chunks[&seq] = receipt;
        sqlx::query("UPDATE zuse_rust_launch_assets SET chunks=$3,updated_at=$4 WHERE workspace_id=$1 AND asset_id=$2").bind(&workspace).bind(&asset).bind(chunks).bind(at).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(json!({"workspaceId":workspace,"assetId":asset,"seq":request.seq}))
    }
    pub async fn launch_asset_commit(
        &self,
        access: &Access,
        request: Commit,
        at: i64,
    ) -> Result<Value, ApiError> {
        if !wire::supported_mime(&request.mime_type)
            || request.size_bytes == 0
            || request.size_bytes > wire::MAX_BYTES
        {
            return Err(invalid());
        }
        let name = request
            .original_name
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or("")
            .trim();
        if name.is_empty() || name.len() > 255 || name.chars().any(char::is_control) {
            return Err(invalid());
        }
        let (mut tx, workspace) = self
            .launch_stage(access, &request.idempotency_key, at)
            .await?;
        let asset = self.asset_id(&access.owner_id, &workspace, &request.upload_id)?;
        let chunks:Option<Value>=sqlx::query_scalar("SELECT chunks FROM zuse_rust_launch_assets WHERE workspace_id=$1 AND asset_id=$2 FOR UPDATE").bind(&workspace).bind(&asset).fetch_optional(&mut *tx).await?;
        let chunks = chunks.ok_or_else(|| ApiError::conflict("launch_asset_incomplete"))?;
        let count = request.size_bytes.div_ceil(wire::CHUNK_BYTES);
        if chunks.as_object().is_none_or(|m| m.len() != count) {
            return Err(ApiError::conflict("launch_asset_incomplete"));
        }
        let objects = self.transcript_objects.as_ref().ok_or_else(failed)?;
        let mut hasher = Sha256::new();
        let mut total = 0;
        for seq in 0..count {
            let key = format!("{}/{seq}.json", prefix(&workspace, &asset));
            let sealed = objects.get_archive(&key).await?.ok_or_else(failed)?;
            let plaintext = self
                .master
                .open_api(
                    &context(
                        &access.owner_id,
                        &workspace,
                        &asset,
                        &format!("\nchunk\n{seq}"),
                    ),
                    std::str::from_utf8(&sealed).map_err(|_| failed())?,
                )
                .map_err(|_| failed())?;
            let bytes = URL_SAFE_NO_PAD.decode(&*plaintext).map_err(|_| failed())?;
            let expected = if seq + 1 < count {
                wire::CHUNK_BYTES
            } else {
                request.size_bytes - seq * wire::CHUNK_BYTES
            };
            if bytes.len() != expected || chunks[seq.to_string()]["sha256"] != digest(&bytes) {
                return Err(ApiError::conflict("launch_asset_integrity_failed"));
            }
            total += bytes.len();
            hasher.update(&bytes);
        }
        if total != request.size_bytes
            || URL_SAFE_NO_PAD.encode(hasher.finalize()) != request.sha256
        {
            return Err(ApiError::conflict("launch_asset_integrity_failed"));
        }
        let asset = Asset {
            asset_id: asset,
            mime_type: request.mime_type,
            original_name: name.into(),
            size_bytes: total,
            sha256: request.sha256,
        };
        let canonical = json!({"version":2,"asset":asset,"chunks":count});
        let aad = context(&access.owner_id, &workspace, &asset.asset_id, "");
        let key = format!("{}.json", prefix(&workspace, &asset.asset_id));
        let sealed = if let Some(existing) = objects.get_archive(&key).await? {
            let encoded = String::from_utf8(existing).map_err(|_| failed())?;
            let plain = self.master.open_api(&aad, &encoded).map_err(|_| failed())?;
            let prior: Value = serde_json::from_slice(&plain).map_err(|_| failed())?;
            if prior != canonical {
                return Err(ApiError::conflict("launch_asset_manifest_conflict"));
            }
            encoded
        } else {
            let sealed = self
                .master
                .seal_api(&aad, &canonical.to_string())
                .map_err(|_| failed())?;
            objects.put_archive(&key, sealed.as_bytes()).await?;
            sealed
        };
        // Keep private filenames/metadata encrypted in PostgreSQL as well as R2.
        let envelope: Value = serde_json::from_str(&sealed).map_err(|_| failed())?;
        sqlx::query("UPDATE zuse_rust_launch_assets SET manifest=$3,state='ready',updated_at=$4 WHERE workspace_id=$1 AND asset_id=$2").bind(&workspace).bind(&asset.asset_id).bind(envelope).bind(at).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(json!({"workspaceId":workspace,"asset":asset}))
    }
}
impl RuntimeControl {
    pub(super) fn verify_launch_asset_rows(
        &self,
        owner: &str,
        workspace: &str,
        assets: &[Asset],
        rows: &[Value],
    ) -> Result<(), ApiError> {
        let mut ids = std::collections::HashSet::new();
        for asset in assets {
            if !ids.insert(&asset.asset_id) {
                return Err(invalid());
            }
            let encoded = rows
                .iter()
                .find(|row| row["asset_id"] == asset.asset_id)
                .and_then(|row| row.get("manifest"))
                .filter(|v| !v.is_null())
                .ok_or_else(|| ApiError::conflict("launch_asset_incomplete"))?;
            let plain = self
                .master
                .open_api(
                    &context(owner, workspace, &asset.asset_id, ""),
                    &encoded.to_string(),
                )
                .map_err(|_| failed())?;
            let expected: Value = serde_json::from_slice(&plain).map_err(|_| failed())?;
            let metadata: Asset =
                serde_json::from_value(expected["asset"].clone()).map_err(|_| failed())?;
            if metadata != *asset {
                return Err(ApiError::conflict("launch_asset_manifest_conflict"));
            }
        }
        Ok(())
    }
    pub(super) async fn verify_launch_assets(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        owner: &str,
        workspace: &str,
        assets: &[Asset],
    ) -> Result<(), ApiError> {
        let mut ids = std::collections::HashSet::new();
        for asset in assets {
            if !ids.insert(&asset.asset_id) {
                return Err(invalid());
            }
            // FOR SHARE: a concurrent release waits for this admission's
            // binding, and this admission never binds an asset being released.
            let row:Option<(Option<Value>,String)>=sqlx::query_as("SELECT manifest,state FROM zuse_rust_launch_assets WHERE workspace_id=$1 AND asset_id=$2 FOR SHARE").bind(workspace).bind(&asset.asset_id).fetch_optional(&mut **tx).await?;
            if row
                .as_ref()
                .is_some_and(|(_, state)| matches!(state.as_str(), "releasing" | "released"))
            {
                return Err(released());
            }
            let encoded = row
                .filter(|(_, state)| state == "ready")
                .and_then(|(manifest, _)| manifest);
            let plain = self
                .master
                .open_api(
                    &context(owner, workspace, &asset.asset_id, ""),
                    &encoded
                        .ok_or_else(|| ApiError::conflict("launch_asset_incomplete"))?
                        .to_string(),
                )
                .map_err(|_| failed())?;
            let expected: Value = serde_json::from_slice(&plain).map_err(|_| failed())?;
            let metadata: Asset =
                serde_json::from_value(expected["asset"].clone()).map_err(|_| failed())?;
            if metadata != *asset {
                return Err(ApiError::conflict("launch_asset_manifest_conflict"));
            }
            let ready:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM zuse_rust_launch_assets WHERE workspace_id=$1 AND asset_id=$2 AND state='ready')").bind(workspace).bind(&asset.asset_id).fetch_one(&mut **tx).await?;
            if !ready {
                return Err(ApiError::conflict("launch_asset_incomplete"));
            }
        }
        Ok(())
    }
    pub async fn runtime_asset(
        &self,
        workspace: &str,
        credential: &str,
        id: &str,
        at: i64,
    ) -> Result<Download, ApiError> {
        let mut tx = self.pool.begin().await?;
        let (row, _, _) = locked_workspace(&mut tx, workspace).await?;
        super::mailbox::runtime_auth(&row, credential, at)?;
        let generation = row["request_config"]["runtimeGeneration"]
            .as_i64()
            .unwrap_or(1);
        let fence = super::mailbox::destruction_fence(&row);
        tx.commit().await?;
        self.scope_access(workspace, None, true).await?;
        let result = self
            .read_asset(required(&row, "account_id")?, workspace, id)
            .await?;
        self.scope_access(workspace, None, true).await?;
        let mut tx = self.pool.begin().await?;
        let (current, current_generation, _) = locked_workspace(&mut tx, workspace).await?;
        super::mailbox::runtime_auth(&current, credential, chrono::Utc::now().timestamp_millis())?;
        if current_generation != generation
            || super::mailbox::destruction_fence(&current) != fence
            || current["account_id"] != row["account_id"]
            || current["request_config"]["sharingPolicy"] != row["request_config"]["sharingPolicy"]
        {
            return Err(ApiError::conflict("cloud_asset_workspace_fenced"));
        }
        tx.commit().await?;
        Ok(result)
    }
    async fn read_asset(
        &self,
        owner: &str,
        workspace: &str,
        id: &str,
    ) -> Result<Download, ApiError> {
        if !valid_id(id) || !id.starts_with("asset_") || !(22..=86).contains(&id.len()) {
            return Err(invalid());
        }
        let objects = self.transcript_objects.as_ref().ok_or_else(failed)?;
        let key = format!("{}.json", prefix(workspace, id));
        let sealed = objects
            .get_asset(&key)
            .await?
            .ok_or_else(|| ApiError::not_found("api_asset_not_found"))?;
        let plain = self
            .master
            .open_asset(
                &context(owner, workspace, id, ""),
                std::str::from_utf8(&sealed).map_err(|_| failed())?,
            )
            .map_err(|_| failed())?;
        let manifest: Value = serde_json::from_slice(&plain).map_err(|_| failed())?;
        let (asset, bytes) = if manifest["version"] == 2 {
            let asset: Asset =
                serde_json::from_value(manifest["asset"].clone()).map_err(|_| failed())?;
            if asset.size_bytes == 0
                || asset.size_bytes > wire::MAX_BYTES
                || manifest["chunks"].as_u64()
                    != Some(asset.size_bytes.div_ceil(wire::CHUNK_BYTES) as u64)
            {
                return Err(failed());
            }
            let mut bytes = Vec::with_capacity(asset.size_bytes);
            for seq in 0..asset.size_bytes.div_ceil(wire::CHUNK_BYTES) {
                let sealed = objects
                    .get_archive(&format!("{}/{seq}.json", prefix(workspace, id)))
                    .await?
                    .ok_or_else(failed)?;
                let plain = self
                    .master
                    .open_api(
                        &context(owner, workspace, id, &format!("\nchunk\n{seq}")),
                        std::str::from_utf8(&sealed).map_err(|_| failed())?,
                    )
                    .map_err(|_| failed())?;
                let chunk = URL_SAFE_NO_PAD.decode(&*plain).map_err(|_| failed())?;
                let expected = (asset.size_bytes - seq * wire::CHUNK_BYTES).min(wire::CHUNK_BYTES);
                if chunk.len() != expected {
                    return Err(failed());
                }
                bytes.extend_from_slice(&chunk);
            }
            (asset, bytes)
        } else if manifest["version"] == 1
            && manifest["accountId"] == owner
            && manifest["workspaceId"] == workspace
        {
            let asset = Asset {
                asset_id: manifest["assetId"].as_str().ok_or_else(failed)?.into(),
                mime_type: manifest["mimeType"].as_str().ok_or_else(failed)?.into(),
                original_name: manifest["originalName"].as_str().ok_or_else(failed)?.into(),
                size_bytes: manifest["sizeBytes"].as_u64().ok_or_else(failed)? as usize,
                sha256: manifest["sha256"].as_str().ok_or_else(failed)?.into(),
            };
            if asset.size_bytes > wire::MAX_BYTES {
                return Err(failed());
            }
            if asset.size_bytes == 0 || asset.size_bytes > wire::MAX_BYTES {
                return Err(failed());
            }
            let bytes = URL_SAFE_NO_PAD
                .decode(manifest["bytes"].as_str().ok_or_else(failed)?)
                .map_err(|_| failed())?;
            (asset, bytes)
        } else {
            return Err(failed());
        };
        if asset.asset_id != id || bytes.len() != asset.size_bytes || digest(&bytes) != asset.sha256
        {
            return Err(ApiError::conflict("launch_asset_integrity_failed"));
        }
        Ok(Download {
            asset,
            bytes: URL_SAFE_NO_PAD.encode(bytes),
        })
    }
}

impl RuntimeControl {
    /// Purge one page of an expired, never-created workspace's uploads. Creation and
    /// upload share this account lock, so cleanup cannot race final launch admission.
    pub async fn sweep_launch_assets_once(&self, at: i64) -> Result<bool, ApiError> {
        let mut tx = self.pool.begin().await?;
        let exists: bool =
            sqlx::query_scalar("SELECT to_regclass('zuse_rust_launch_staging') IS NOT NULL")
                .fetch_one(&mut *tx)
                .await?;
        if !exists {
            return Ok(false);
        }
        let next: Option<(String,String)> = sqlx::query_as("SELECT account_id,workspace_id FROM zuse_rust_launch_staging s WHERE expires_at<=$1 AND account_id=ANY($2) AND NOT EXISTS(SELECT 1 FROM api_cloud_workspaces w WHERE w.workspace_id=s.workspace_id) ORDER BY expires_at LIMIT 1").bind(at).bind(&self.creation_accounts).fetch_optional(&mut *tx).await?;
        let Some((account, workspace)) = next else {
            return Ok(false);
        };
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
            .bind(format!("cloud-create:{account}"))
            .execute(&mut *tx)
            .await?;
        let admitted:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM zuse_rust_launch_staging s WHERE workspace_id=$1 AND expires_at<=$2 AND NOT EXISTS(SELECT 1 FROM api_cloud_workspaces w WHERE w.workspace_id=s.workspace_id))").bind(&workspace).bind(at).fetch_one(&mut *tx).await?;
        if !admitted {
            return Ok(false);
        }
        let objects = self.transcript_objects.as_ref().ok_or_else(failed)?;
        if objects
            .purge_prefix(&format!("workspaces/{workspace}/api-assets/"))
            .await?
        {
            sqlx::query("DELETE FROM zuse_rust_launch_assets WHERE workspace_id=$1")
                .bind(&workspace)
                .execute(&mut *tx)
                .await?;
            sqlx::query("DELETE FROM zuse_rust_launch_staging WHERE workspace_id=$1")
                .bind(&workspace)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(true)
    }
}
