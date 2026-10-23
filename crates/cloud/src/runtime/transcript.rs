//! Runtime ciphertext publication precedes its metadata pointer. No plaintext history in PostgreSQL.
use super::*;
use crate::transcript::{Upload, formatted_key, validate_upload};
impl RuntimeControl {
    pub fn with_transcript_objects(mut self, objects: Arc<dyn crate::objects::Objects>) -> Self {
        self.transcript_objects = Some(objects);
        self
    }
    pub async fn transcript_upload(
        &self,
        id: &str,
        credential: &str,
        upload: Upload,
        page: bool,
        now: i64,
    ) -> Result<Value, ApiError> {
        validate_upload(&upload, page)?;
        let native = upload.payload_format.as_deref() == Some(zuse_cloud_projection::FORMAT);
        let objects = self
            .transcript_objects
            .as_ref()
            .ok_or_else(|| ApiError::unavailable("cloud_transcript_store_unavailable"))?;
        let mut tx = self.pool.begin().await?;
        let (row, generation, _) = locked_workspace(&mut tx, id).await?;
        // Archive quiescence may publish its final projection, but deletion/replacement cannot.
        if matches!(row["state"].as_str(), Some("deleted" | "deleting"))
            || row["desired_state"] == "deleted"
            || row["request_config"]["cloudMailboxLifecyclePending"]["action"] == "delete"
            || row["request_config"]["cloudMailboxLifecycleDelivered"]["action"] == "delete"
            || row["runtime_credential_hash"]
                .as_str()
                .is_none_or(|h| !equal(h, &hash(credential)))
            || row["request_config"]["runtimeCredentialExpiresAtMs"]
                .as_i64()
                .unwrap_or(0)
                <= now
            || !storage_enrolled(&row)
        {
            return Err(ApiError::unauthorized("workspace_runtime_fenced"));
        }
        if page {
            let valid:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM api_cloud_transcript_checkpoints WHERE workspace_id=$1 AND session_id=$2 AND runtime_generation=$3 AND stream_epoch=$4 AND stream_version=$5)").bind(id).bind(&upload.session_id).bind(generation).bind(&upload.cursor.epoch).bind(upload.cursor.version).fetch_one(&mut *tx).await?;
            if !valid {
                return Err(ApiError::conflict("cloud_transcript_cursor_stale"));
            }
        }
        let key = formatted_key(
            id,
            &upload.session_id,
            generation,
            &upload.cursor,
            upload.before_sequence,
            native,
        );
        objects.put(&key, upload.ciphertext.as_bytes()).await?;
        let applied = if page {
            true
        } else {
            sqlx::query("INSERT INTO api_cloud_transcript_checkpoints (workspace_id,session_id,runtime_generation,stream_epoch,stream_version,object_key,ciphertext_sha256,ciphertext_bytes,created_at) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9) ON CONFLICT (workspace_id,session_id) DO UPDATE SET runtime_generation=EXCLUDED.runtime_generation,stream_epoch=EXCLUDED.stream_epoch,stream_version=EXCLUDED.stream_version,object_key=EXCLUDED.object_key,ciphertext_sha256=EXCLUDED.ciphertext_sha256,ciphertext_bytes=EXCLUDED.ciphertext_bytes,created_at=EXCLUDED.created_at WHERE (EXCLUDED.runtime_generation>api_cloud_transcript_checkpoints.runtime_generation AND EXCLUDED.stream_version>=api_cloud_transcript_checkpoints.stream_version) OR (EXCLUDED.runtime_generation=api_cloud_transcript_checkpoints.runtime_generation AND EXCLUDED.stream_epoch=api_cloud_transcript_checkpoints.stream_epoch AND EXCLUDED.stream_version>api_cloud_transcript_checkpoints.stream_version)")
                .bind(id).bind(&upload.session_id).bind(generation).bind(&upload.cursor.epoch).bind(upload.cursor.version).bind(&key).bind(&upload.ciphertext_sha256).bind(upload.ciphertext.len() as i64).bind(now).execute(&mut *tx).await?.rows_affected()==1
        };
        if native && !page && !applied {
            let matches:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM api_cloud_transcript_checkpoints WHERE workspace_id=$1 AND session_id=$2 AND runtime_generation=$3 AND stream_epoch=$4 AND stream_version=$5 AND object_key=$6 AND ciphertext_sha256=$7)")
                .bind(id).bind(&upload.session_id).bind(generation).bind(&upload.cursor.epoch).bind(upload.cursor.version).bind(&key).bind(&upload.ciphertext_sha256).fetch_one(&mut *tx).await?;
            if !matches {
                return Err(ApiError::conflict("cloud_transcript_cursor_stale"));
            }
        }
        tx.commit().await?;
        if page {
            return Ok(json!({"applied":true}));
        }
        Ok(
            json!({"applied":applied,"cursor":upload.cursor,"archiveRequested":row["desired_state"]=="archived"}),
        )
    }
}
