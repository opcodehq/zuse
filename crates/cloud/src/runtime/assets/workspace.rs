//! Compute-independent task assets. Each object write is checkpointed before
//! dispatch, then reauthorized after I/O. Reads and membership checks run outside
//! SQL locks; scoped shared write fences exclude destructive cleanup.
use super::*;
impl RuntimeControl {
    async fn workspace_asset_authorize(
        &self,
        id: &str,
        access: &Access,
        edit: bool,
    ) -> Result<Value, ApiError> {
        let fresh = self
            .scope_access(id, Some(&access.actor.account_id), edit)
            .await?;
        let access = fresh.as_ref().map_or(access, |(access, _)| access);
        let row:Value=sqlx::query_scalar("SELECT to_jsonb(w) FROM api_cloud_workspaces w WHERE workspace_id=$1 AND account_id=$2").bind(id).bind(&access.owner_id).fetch_optional(&self.pool).await?.ok_or_else(||ApiError::not_found("cloud_workspace_not_found"))?;
        self.workspace_asset_row(&row, access, edit)?;
        if fresh
            .as_ref()
            .is_some_and(|(_, policy)| row["request_config"]["sharingPolicy"] != *policy)
        {
            return Err(ApiError::conflict("workspace_sharing_changed"));
        }
        Ok(row)
    }
    fn workspace_asset_row(
        &self,
        row: &Value,
        access: &Access,
        edit: bool,
    ) -> Result<(), ApiError> {
        if row["account_id"] != access.owner_id {
            return Err(ApiError::not_found("cloud_workspace_not_found"));
        }
        let permission = access.workspace_permission(&access.owner_id, &row["request_config"])?;
        if edit && permission != crate::access::Permission::Edit {
            return Err(ApiError::forbidden("workspace_access_denied"));
        }
        if blocked(row) || row["desired_state"] == "deleted" {
            return Err(ApiError::not_found("cloud_workspace_not_found"));
        }
        if edit && !crate::runtime::task_commands::command_mode(row) {
            return Err(ApiError::conflict(
                "server_command_admission_update_required",
            ));
        }
        if edit
            && (row["desired_state"] == "archived"
                || row["request_config"]["cloudMailboxLifecyclePending"]["action"] == "archive")
        {
            return Err(ApiError::conflict("cloud_workspace_unavailable"));
        }
        Ok(())
    }
    async fn workspace_asset_locked<'a>(
        &self,
        id: &str,
        access: &Access,
        fence: i64,
        tx: &mut Transaction<'a, Postgres>,
    ) -> Result<Value, ApiError> {
        let (row, _, _) = locked_workspace(tx, id).await?;
        self.workspace_asset_row(&row, access, true)?;
        if super::super::mailbox::destruction_fence(&row) != fence {
            return Err(ApiError::conflict("cloud_asset_workspace_fenced"));
        }
        Ok(row)
    }
    pub async fn workspace_asset_chunk(
        &self,
        id: &str,
        access: &Access,
        request: Chunk,
        now: i64,
    ) -> Result<Value, ApiError> {
        if request.idempotency_key != format!("workspace:{id}")
            || request.seq >= wire::MAX_BYTES.div_ceil(wire::CHUNK_BYTES)
            || request.bytes.len() > wire::CHUNK_BYTES.div_ceil(3) * 4
        {
            return Err(invalid());
        }
        let bytes = zeroize::Zeroizing::new(
            URL_SAFE_NO_PAD
                .decode(&request.bytes)
                .map_err(|_| invalid())?,
        );
        if bytes.is_empty() || bytes.len() > wire::CHUNK_BYTES {
            return Err(invalid());
        }
        let early = self.workspace_asset_authorize(id, access, true).await?;
        let fence = super::super::mailbox::destruction_fence(&early);
        let asset = self.asset_id(&access.owner_id, id, &request.upload_id)?;
        let mut tx = self.pool.begin().await?;
        self.workspace_asset_locked(id, access, fence, &mut tx)
            .await?;
        let schema: bool =
            sqlx::query_scalar("SELECT to_regclass('zuse_rust_task_asset_chunks') IS NOT NULL")
                .fetch_one(&mut *tx)
                .await?;
        if !schema {
            return Err(ApiError::unavailable("cloud_task_asset_schema_required"));
        }
        let pending:i64=sqlx::query_scalar("SELECT COUNT(*) FROM zuse_rust_launch_assets WHERE workspace_id=$1 AND state='uploading'").bind(id).fetch_one(&mut *tx).await?;
        let exists:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM zuse_rust_launch_assets WHERE workspace_id=$1 AND asset_id=$2)").bind(id).bind(&asset).fetch_one(&mut *tx).await?;
        if pending >= 16 && !exists {
            return Err(ApiError::conflict("cloud_task_upload_limit"));
        }
        sqlx::query("INSERT INTO zuse_rust_launch_assets(workspace_id,asset_id,state,created_at,updated_at) VALUES($1,$2,'uploading',$3,$3) ON CONFLICT DO NOTHING").bind(id).bind(&asset).bind(now).execute(&mut *tx).await?;
        let prior:Option<(i32,String,Option<String>,bool)>=sqlx::query_as("SELECT size_bytes,sha256,ciphertext,ready FROM zuse_rust_task_asset_chunks WHERE workspace_id=$1 AND asset_id=$2 AND chunk_sequence=$3 FOR UPDATE").bind(id).bind(&asset).bind(request.seq as i32).fetch_optional(&mut *tx).await?;
        let hash = digest(&bytes);
        let aad = context(
            &access.owner_id,
            id,
            &asset,
            &format!("\nchunk\n{}", request.seq),
        );
        let (sealed, confirmed) = if let Some((size, sha, sealed, ready)) = prior {
            if size != bytes.len() as i32 || sha != hash {
                return Err(ApiError::conflict("launch_asset_chunk_conflict"));
            }
            (sealed, ready)
        } else {
            let state: String = sqlx::query_scalar(
                "SELECT state FROM zuse_rust_launch_assets WHERE workspace_id=$1 AND asset_id=$2",
            )
            .bind(id)
            .bind(&asset)
            .fetch_one(&mut *tx)
            .await?;
            if state == "ready" {
                return Err(ApiError::conflict("launch_asset_already_committed"));
            }
            let sealed = self
                .master
                .seal_api(&aad, &URL_SAFE_NO_PAD.encode(&*bytes))
                .map_err(|_| failed())?;
            sqlx::query("INSERT INTO zuse_rust_task_asset_chunks(workspace_id,asset_id,chunk_sequence,size_bytes,sha256,ciphertext,ready,updated_at) VALUES($1,$2,$3,$4,$5,$6,false,$7)").bind(id).bind(&asset).bind(request.seq as i32).bind(bytes.len() as i32).bind(&hash).bind(&sealed).bind(now).execute(&mut *tx).await?;
            (Some(sealed), false)
        };
        tx.commit().await?;
        self.workspace_asset_authorize(id, access, true).await?;
        // One bounded object writer leaves runtime-control pool capacity for
        // interactive commands and new task admission. Acquire before SQL.
        let _permit = self
            .object_writes
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| failed())?;
        let mut io = self.pool.begin().await?;
        let (current, _, _) = locked_workspace_with(&mut io, id, WorkspaceLock::Artifact).await?;
        self.workspace_asset_row(&current, access, true)?;
        if super::super::mailbox::destruction_fence(&current) != fence {
            return Err(ApiError::conflict("cloud_asset_workspace_fenced"));
        }
        let objects = self.transcript_objects.as_ref().ok_or_else(failed)?;
        let key = format!("{}/{seq}.json", prefix(id, &asset), seq = request.seq);
        let intent=super::super::object_writes::ObjectWriteIntent::prepare(&mut io,&current,&key,"asset-chunk",now).await?;
        intent.authorize_result(&early)?;
        io.commit().await?;
        let authorized=self.workspace_asset_authorize(id,access,true).await?;
        intent.authorize_result(&authorized)?;
        // A durable attempt owns this possible PUT even if cancellation drops
        // the HTTP future and a late remote completion follows workspace delete.
        if let Some(stored) = objects.get_archive(&key).await? {
            let plain = self
                .master
                .open_api(&aad, std::str::from_utf8(&stored).map_err(|_| failed())?)
                .map_err(|_| failed())?;
            if URL_SAFE_NO_PAD
                .decode(&*plain)
                .map_err(|_| failed())?
                .as_slice()
                != bytes.as_slice()
            {
                return Err(ApiError::conflict("launch_asset_chunk_conflict"));
            }
        } else {
            if confirmed {
                return Err(ApiError::conflict("cloud_task_asset_object_missing"));
            }
            objects
                .put_archive(&key, sealed.as_ref().ok_or_else(failed)?.as_bytes())
                .await?;
        }
        let authorized=self.workspace_asset_authorize(id,access,true).await?;
        intent.authorize_result(&authorized)?;
        let mut tx = self.pool.begin().await?;
        let current=self.workspace_asset_locked(id, access, fence, &mut tx).await?;
        if current["request_config"]["sharingPolicy"]!=authorized["request_config"]["sharingPolicy"]{return Err(ApiError::conflict("workspace_sharing_changed"));}
        let affected=sqlx::query("UPDATE zuse_rust_task_asset_chunks SET ready=true,ciphertext=NULL,updated_at=$6 WHERE workspace_id=$1 AND asset_id=$2 AND chunk_sequence=$3 AND sha256=$4 AND size_bytes=$5").bind(id).bind(&asset).bind(request.seq as i32).bind(&hash).bind(bytes.len() as i32).bind(now).execute(&mut *tx).await?;
        if affected.rows_affected() != 1 {
            return Err(ApiError::conflict("cloud_task_asset_receipt_changed"));
        }
        sqlx::query("UPDATE zuse_rust_launch_assets SET chunks=jsonb_set(chunks,ARRAY[$3::text],$4::jsonb),updated_at=$5 WHERE workspace_id=$1 AND asset_id=$2").bind(id).bind(&asset).bind(request.seq.to_string()).bind(json!({"size":bytes.len(),"sha256":hash})).bind(now).execute(&mut *tx).await?;
        intent.commit(&mut tx,&current).await?;
        tx.commit().await?;
        Ok(json!({"workspaceId":id,"assetId":asset,"seq":request.seq}))
    }
}
impl RuntimeControl {
    /// Bounded browser/native reads never acquire an execution environment. The
    /// committed encrypted PG manifest avoids an extra R2 header fetch; only the
    /// encrypted chunks intersecting this page are downloaded.
    pub async fn workspace_asset_read(
        &self,
        id: &str,
        access: &Access,
        asset: &str,
        offset: usize,
    ) -> Result<Value, ApiError> {
        use base64::engine::general_purpose::STANDARD;
        if !valid_id(asset) || !asset.starts_with("asset_") {
            return Err(invalid());
        }
        let before = self.workspace_asset_authorize(id, access, false).await?;
        let fence = super::super::mailbox::destruction_fence(&before);
        let manifest:Value=sqlx::query_scalar("SELECT manifest FROM zuse_rust_launch_assets WHERE workspace_id=$1 AND asset_id=$2 AND state='ready'").bind(id).bind(asset).fetch_optional(&self.pool).await?.ok_or_else(||ApiError::not_found("api_asset_not_found"))?;
        let owner = required(&before, "account_id")?;
        let plain = self
            .master
            .open_api(&context(owner, id, asset, ""), &manifest.to_string())
            .map_err(|_| failed())?;
        let canonical: Value = serde_json::from_slice(&plain).map_err(|_| failed())?;
        let metadata: Asset =
            serde_json::from_value(canonical["asset"].clone()).map_err(|_| failed())?;
        if canonical["version"] != 2
            || metadata.asset_id != asset
            || metadata.size_bytes == 0
            || metadata.size_bytes > wire::MAX_BYTES
            || canonical["chunks"].as_u64()
                != Some(metadata.size_bytes.div_ceil(wire::CHUNK_BYTES) as u64)
        {
            return Err(failed());
        }
        if offset > metadata.size_bytes {
            return Err(ApiError::bad_request("cloud_asset_offset_invalid"));
        }
        let end = offset.saturating_add(256 * 1024).min(metadata.size_bytes);
        let mut bytes = zeroize::Zeroizing::new(Vec::with_capacity(end - offset));
        let objects = self.transcript_objects.as_ref().ok_or_else(failed)?;
        if end > offset {
            for seq in offset / wire::CHUNK_BYTES..=(end - 1) / wire::CHUNK_BYTES {
                let sealed = objects
                    .get_archive(&format!("{}/{seq}.json", prefix(id, asset)))
                    .await?
                    .ok_or_else(failed)?;
                let plain = self
                    .master
                    .open_api(
                        &context(owner, id, asset, &format!("\nchunk\n{seq}")),
                        std::str::from_utf8(&sealed).map_err(|_| failed())?,
                    )
                    .map_err(|_| failed())?;
                let chunk =
                    zeroize::Zeroizing::new(URL_SAFE_NO_PAD.decode(&*plain).map_err(|_| failed())?);
                let start = seq * wire::CHUNK_BYTES;
                if chunk.len() != (metadata.size_bytes - start).min(wire::CHUNK_BYTES) {
                    return Err(failed());
                }
                bytes.extend_from_slice(
                    &chunk[offset.saturating_sub(start)..(end - start).min(chunk.len())],
                );
            }
        }
        // Recheck after object I/O, with no SQL transaction held across HTTP.
        let after = self.workspace_asset_authorize(id, access, false).await?;
        if super::super::mailbox::destruction_fence(&after) != fence
            || after["account_id"] != before["account_id"]
            || after["request_config"]["runtimeGeneration"]!=before["request_config"]["runtimeGeneration"]
            || after["request_config"]["rustRuntimeStorage"]["storageIncarnationId"]!=before["request_config"]["rustRuntimeStorage"]["storageIncarnationId"]
            || (!before["provider_sandbox_id"].is_null()&&after["provider_sandbox_id"]!=before["provider_sandbox_id"])
        {
            return Err(ApiError::conflict("cloud_asset_workspace_fenced"));
        }
        Ok(
            json!({"asset":metadata,"name":metadata.original_name,"mimeType":metadata.mime_type,"data":STANDARD.encode(&*bytes),"nextOffset":end,"done":end==metadata.size_bytes}),
        )
    }
    pub async fn workspace_asset_commit(
        &self,
        id: &str,
        access: &Access,
        request: Commit,
        now: i64,
    ) -> Result<Value, ApiError> {
        if request.idempotency_key != format!("workspace:{id}")
            || !wire::supported_mime(&request.mime_type)
            || request.size_bytes == 0
            || request.size_bytes > wire::MAX_BYTES
            || request.sha256.len() != 43
            || !request
                .sha256
                .bytes()
                .all(|v| v.is_ascii_alphanumeric() || b"_-".contains(&v))
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
        let early = self.workspace_asset_authorize(id, access, true).await?;
        let fence = super::super::mailbox::destruction_fence(&early);
        let asset_id = self.asset_id(&access.owner_id, id, &request.upload_id)?;
        let expected = Asset {
            asset_id: asset_id.clone(),
            mime_type: request.mime_type,
            original_name: name.into(),
            size_bytes: request.size_bytes,
            sha256: request.sha256,
        };
        let mut tx = self.pool.begin().await?;
        self.workspace_asset_locked(id, access, fence, &mut tx)
            .await?;
        let item:Value=sqlx::query_scalar("SELECT to_jsonb(a) FROM zuse_rust_launch_assets a WHERE workspace_id=$1 AND asset_id=$2 FOR UPDATE").bind(id).bind(&asset_id).fetch_optional(&mut *tx).await?.ok_or_else(||ApiError::conflict("launch_asset_incomplete"))?;
        let count = expected.size_bytes.div_ceil(wire::CHUNK_BYTES);
        let chunks = item["chunks"].clone();
        if chunks.as_object().is_none_or(|v| v.len() != count) {
            return Err(ApiError::conflict("launch_asset_incomplete"));
        }
        let canonical = json!({"version":2,"asset":expected,"chunks":count});
        let aad = context(&access.owner_id, id, &asset_id, "");
        let sealed = if !item["manifest"].is_null() {
            let encoded = item["manifest"].to_string();
            let plain = self.master.open_api(&aad, &encoded).map_err(|_| failed())?;
            if serde_json::from_slice::<Value>(&plain).map_err(|_| failed())? != canonical {
                return Err(ApiError::conflict("launch_asset_manifest_conflict"));
            }
            encoded
        } else {
            let encoded = self
                .master
                .seal_api(&aad, &canonical.to_string())
                .map_err(|_| failed())?;
            sqlx::query("UPDATE zuse_rust_launch_assets SET manifest=$3,updated_at=$4 WHERE workspace_id=$1 AND asset_id=$2").bind(id).bind(&asset_id).bind(serde_json::from_str::<Value>(&encoded).map_err(|_|failed())?).bind(now).execute(&mut *tx).await?;
            encoded
        };
        tx.commit().await?;
        let objects = self.transcript_objects.as_ref().ok_or_else(failed)?;
        // Hash original plaintext in bounded chunks. Immutable object receipts
        // were checkpointed before each PUT; no PG lock spans these reads.
        let mut hasher = Sha256::new();
        let mut total = 0usize;
        for sequence in 0..count {
            let stored = objects
                .get_archive(&format!("{}/{sequence}.json", prefix(id, &asset_id)))
                .await?
                .ok_or_else(|| ApiError::conflict("launch_asset_incomplete"))?;
            let bytes = self
                .master
                .open_api(
                    &context(
                        &access.owner_id,
                        id,
                        &asset_id,
                        &format!("\nchunk\n{sequence}"),
                    ),
                    std::str::from_utf8(&stored).map_err(|_| failed())?,
                )
                .map_err(|_| failed())?;
            let bytes =
                zeroize::Zeroizing::new(URL_SAFE_NO_PAD.decode(&*bytes).map_err(|_| failed())?);
            let size = (expected.size_bytes - sequence * wire::CHUNK_BYTES).min(wire::CHUNK_BYTES);
            if bytes.len() != size
                || chunks[sequence.to_string()]["size"] != size
                || chunks[sequence.to_string()]["sha256"] != digest(&bytes)
            {
                return Err(ApiError::conflict("launch_asset_integrity_failed"));
            }
            hasher.update(&*bytes);
            total += bytes.len();
        }
        if total != expected.size_bytes
            || URL_SAFE_NO_PAD.encode(hasher.finalize()) != expected.sha256
        {
            return Err(ApiError::conflict("launch_asset_integrity_failed"));
        }
        self.workspace_asset_authorize(id, access, true).await?;
        // The immutable write intent, rather than a SQL lock, owns a remote
        // completion that can outlive cancellation or workspace deletion.
        let _permit = self
            .object_writes
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| failed())?;
        let mut io = self.pool.begin().await?;
        let (row, _, _) = locked_workspace_with(&mut io, id, WorkspaceLock::Artifact).await?;
        self.workspace_asset_row(&row, access, true)?;
        if super::super::mailbox::destruction_fence(&row) != fence {
            return Err(ApiError::conflict("cloud_asset_workspace_fenced"));
        }
        let key=format!("{}.json",prefix(id,&asset_id));
        let intent=super::super::object_writes::ObjectWriteIntent::prepare(&mut io,&row,&key,"asset-manifest",now).await?;
        intent.authorize_result(&early)?;
        io.commit().await?;
        let authorized=self.workspace_asset_authorize(id,access,true).await?;
        intent.authorize_result(&authorized)?;
        objects.put_archive(&key,sealed.as_bytes()).await?;
        let authorized=self.workspace_asset_authorize(id,access,true).await?;
        intent.authorize_result(&authorized)?;
        let mut tx = self.pool.begin().await?;
        let current=self.workspace_asset_locked(id, access, fence, &mut tx).await?;
        if current["request_config"]["sharingPolicy"]!=authorized["request_config"]["sharingPolicy"]{return Err(ApiError::conflict("workspace_sharing_changed"));}
        let changed=sqlx::query("UPDATE zuse_rust_launch_assets SET state='ready',updated_at=$4 WHERE workspace_id=$1 AND asset_id=$2 AND manifest=$3").bind(id).bind(&asset_id).bind(serde_json::from_str::<Value>(&sealed).map_err(|_|failed())?).bind(now).execute(&mut *tx).await?;
        if changed.rows_affected() != 1 {
            return Err(ApiError::conflict("launch_asset_manifest_conflict"));
        }
        intent.commit(&mut tx,&current).await?;
        tx.commit().await?;
        Ok(json!({"workspaceId":id,"asset":expected}))
    }
}
