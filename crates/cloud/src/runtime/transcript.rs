//! Runtime ciphertext publication precedes its metadata pointer. Bounded opening heads also live in PostgreSQL.
use super::*;
use crate::transcript::{Upload, formatted_key, validate_upload};
impl RuntimeControl {
    pub fn with_transcript_objects(mut self, objects: Arc<dyn crate::objects::Objects>) -> Self {
        self.transcript_objects = Some(objects);
        self
    }
    #[tracing::instrument(name="cloud_runtime_request",skip_all,fields(workspace=%id,operation="transcript_upload"))]
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
        let unlocked_put = self.transcript_intents_available().await?;
        let _permit = if unlocked_put {
            Some(
                self.object_writes
                    .clone()
                    .acquire_owned()
                    .await
                    .map_err(|_| ApiError::unavailable("cloud_transcript_store_unavailable"))?,
            )
        } else {
            None
        };
        let mut tx = self.pool.begin().await?;
        let (mut row, generation, _) = locked_workspace(&mut tx, id).await?;
        authorize_projection(&row, credential, now)?;
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
        let intent = if unlocked_put && super::task_commands::command_mode(&row) {
            let intent = super::object_writes::ObjectWriteIntent::prepare(
                &mut tx,
                &row,
                &key,
                "transcript",
                chrono::Utc::now().timestamp_millis(),
            )
            .await?;
            tx.commit().await?;
            // The durable intent, rather than a workspace SQL lock, owns any
            // late PUT. Deletion can proceed and its reconciler resweeps it.
            put_projection(objects.as_ref(), &key, upload.ciphertext.as_bytes()).await?;
            tx = self.pool.begin().await?;
            row = locked_workspace(&mut tx, id).await?.0;
            authorize_projection(&row, credential, chrono::Utc::now().timestamp_millis())?;
            intent.authorize_result(&row)?;
            if page {
                let valid:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM api_cloud_transcript_checkpoints WHERE workspace_id=$1 AND session_id=$2 AND runtime_generation=$3 AND stream_epoch=$4 AND stream_version=$5)").bind(id).bind(&upload.session_id).bind(generation).bind(&upload.cursor.epoch).bind(upload.cursor.version).fetch_one(&mut *tx).await?;
                if !valid {
                    return Err(ApiError::conflict("cloud_transcript_cursor_stale"));
                }
            }
            Some(intent)
        } else {
            put_projection(objects.as_ref(), &key, upload.ciphertext.as_bytes()).await?;
            None
        };
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
        if applied && !page {
            let hot: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM information_schema.columns WHERE table_schema=current_schema() AND table_name='api_cloud_transcript_checkpoints' AND column_name='rust_hot_head')").fetch_one(&mut *tx).await?;
            if hot {
                let head = (upload.ciphertext.len() <= crate::transcript::HOT_HEAD_LIMIT).then(||json!({"runtimeGeneration":generation,"epoch":upload.cursor.epoch,"version":upload.cursor.version,"sha256":upload.ciphertext_sha256,"ciphertext":upload.ciphertext}));
                sqlx::query("UPDATE api_cloud_transcript_checkpoints SET rust_hot_head=$3 WHERE workspace_id=$1 AND session_id=$2")
                    .bind(id).bind(&upload.session_id).bind(head).execute(&mut *tx).await?;
            }
        }
        if native && (page || applied) {
            self.save_task_history_tx(&mut tx, &row, &upload, now)
                .await?;
        }
        if let Some(intent) = intent {
            intent.commit(&mut tx, &row).await?;
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

#[cfg(test)]
mod tests;

fn authorize_projection(row: &Value, credential: &str, now: i64) -> Result<(), ApiError> {
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
    Ok(())
}
async fn put_projection(
    objects: &dyn crate::objects::Objects,
    key: &str,
    bytes: &[u8],
) -> Result<(), ApiError> {
    let started = std::time::Instant::now();
    let result = objects.put(key, bytes).await;
    tracing::info!(
        phase = "transcript-object-put",
        duration_ms = started.elapsed().as_millis() as u64,
        succeeded = result.is_ok(),
        "Cloud object publication phase"
    );
    result
}
