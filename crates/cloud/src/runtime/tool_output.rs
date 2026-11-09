//! Canonical references authorize history-only tool reads. Output never wakes a computer.
use super::*;
use crate::access::Access;
use sqlx::Row;
use zuse_cloud_runtime::tool_output::{CHUNK_BYTES, ToolOutputChunk, valid_ref};

pub(super) async fn allow_refs(
    tx: &mut Transaction<'_, Postgres>,
    id: &str,
    session: &str,
    entries: &[Value],
) -> Result<(), ApiError> {
    let mut references = std::collections::BTreeSet::new();
    for entry in entries {
        for part in entry["parts"].as_array().into_iter().flatten() {
            if part["kind"] != "tool" || part["resolved"] != true {
                continue;
            }
            for key in ["outputRef", "diffRef"] {
                if let Some(reference) = part[key].as_str().filter(|r| {
                    valid_ref(r) && r.split_once('/').is_some_and(|(chat, _)| chat == session)
                }) {
                    references.insert(reference);
                }
            }
        }
    }
    if !references.is_empty() {
        let references: Vec<&str> = references.into_iter().collect();
        sqlx::query("INSERT INTO zuse_rust_tool_output_refs(workspace_id,blob_ref,session_id) SELECT $1,reference,$3 FROM unnest($2::text[]) AS refs(reference) ON CONFLICT DO NOTHING")
            .bind(id).bind(references).bind(session).execute(&mut **tx).await?;
    }
    Ok(())
}
fn context(owner: &str, id: &str, reference: &str, offset: usize) -> String {
    format!("tool-output:{owner}:{id}:{reference}:{offset}")
}

impl RuntimeControl {
    pub async fn tool_output_upload(
        &self,
        id: &str,
        credential: &str,
        chunk: ToolOutputChunk,
        now: i64,
    ) -> Result<Value, ApiError> {
        if !chunk.valid() {
            return Err(ApiError::bad_request("invalid_tool_output_chunk"));
        }
        self.runtime_scope_access(id, credential, now).await?;
        let mut tx = self.pool.begin().await?;
        let (row, _, _) = locked_workspace(&mut tx, id).await?;
        super::mailbox::runtime_auth(&row, credential, now)?;
        let allowed:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM zuse_rust_tool_output_refs WHERE workspace_id=$1 AND blob_ref=$2)")
            .bind(id).bind(&chunk.blob_ref).fetch_one(&mut *tx).await?;
        if !allowed {
            return Err(ApiError::conflict("tool_output_reference_not_published"));
        }
        let total:Option<i64>=sqlx::query_scalar("SELECT total_bytes FROM zuse_rust_tool_output_chunks WHERE workspace_id=$1 AND blob_ref=$2 LIMIT 1")
            .bind(id).bind(&chunk.blob_ref).fetch_optional(&mut *tx).await?;
        if total.is_some_and(|n| n != chunk.total_bytes as i64) {
            return Err(ApiError::conflict("tool_output_immutable_conflict"));
        }
        if chunk.offset > 0 {
            let prior:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM zuse_rust_tool_output_chunks WHERE workspace_id=$1 AND blob_ref=$2 AND next_offset=$3 AND ready)")
                .bind(id).bind(&chunk.blob_ref).bind(chunk.offset as i64).fetch_one(&mut *tx).await?;
            if !prior {
                return Err(ApiError::conflict("tool_output_cursor_not_published"));
            }
        }
        let owner = row["account_id"].as_str().ok_or_else(invalid)?;
        let context = context(owner, id, &chunk.blob_ref, chunk.offset);
        let plain = serde_json::to_string(&chunk).map_err(|_| invalid())?;
        let fingerprint = self.master.digest_api(&context, &plain);
        let prior=sqlx::query("SELECT fingerprint,ciphertext,object_key,ready FROM zuse_rust_tool_output_chunks WHERE workspace_id=$1 AND blob_ref=$2 AND byte_offset=$3")
            .bind(id).bind(&chunk.blob_ref).bind(chunk.offset as i64).fetch_optional(&mut *tx).await?;
        let (ciphertext, object_key) = if let Some(prior) = prior {
            if prior.get::<String, _>("fingerprint") != fingerprint {
                return Err(ApiError::conflict("tool_output_immutable_conflict"));
            }
            if prior.get::<bool, _>("ready") {
                return Ok(json!({"stored":true}));
            }
            (
                prior.get::<String, _>("ciphertext"),
                prior.get::<Option<String>, _>("object_key"),
            )
        } else {
            let ciphertext = self
                .master
                .seal_api(&context, &plain)
                .map_err(|_| invalid())?;
            let object = if chunk.total_bytes > 128 * 1024 {
                if self.transcript_objects.is_none() {
                    return Err(ApiError::unavailable("cloud_tool_output_store_unavailable"));
                }
                Some(format!(
                    "workspaces/{}/tool-output/{}",
                    crate::transcript::component(id),
                    fingerprint
                ))
            } else {
                None
            };
            sqlx::query("INSERT INTO zuse_rust_tool_output_chunks(workspace_id,blob_ref,byte_offset,next_offset,total_bytes,fingerprint,ciphertext,object_key,ready) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9)")
                .bind(id).bind(&chunk.blob_ref).bind(chunk.offset as i64).bind(chunk.next_offset as i64).bind(chunk.total_bytes as i64).bind(&fingerprint).bind(&ciphertext).bind(&object).bind(object.is_none()).execute(&mut *tx).await?;
            (ciphertext, object)
        };
        let canonical = super::task_commands::command_mode(&row);
        tx.commit().await?;
        if let Some(object) = object_key {
            if canonical {
                self.tool_output_put_v2(id, credential, &chunk, &fingerprint, &ciphertext, &object)
                    .await?;
                return Ok(json!({"stored":true}));
            }
            // Commit the encrypted retry receipt first. Reuse its exact ciphertext
            // for immutable-object retries, then hold the workspace fence across
            // put+commit so deletion cannot purge before a late upload appears.
            let mut tx = self.pool.begin().await?;
            let (row, _, _) = locked_workspace(&mut tx, id).await?;
            super::mailbox::runtime_auth(&row, credential, chrono::Utc::now().timestamp_millis())?;
            self.transcript_objects
                .as_ref()
                .ok_or_else(invalid)?
                .put(&object, ciphertext.as_bytes())
                .await?;
            sqlx::query("UPDATE zuse_rust_tool_output_chunks SET ready=true,ciphertext=NULL WHERE workspace_id=$1 AND blob_ref=$2 AND byte_offset=$3 AND fingerprint=$4")
                .bind(id).bind(&chunk.blob_ref).bind(chunk.offset as i64).bind(&fingerprint).execute(&mut *tx).await?;
            tx.commit().await?;
        }
        Ok(json!({"stored":true}))
    }

    /// A durable intent replaces the SQL lock across remote I/O. An aborted
    /// request may leave R2 executing, so only an acknowledged physical attempt
    /// can remove its own intent in the atomic readiness commit.
    async fn tool_output_put_v2(
        &self,
        id: &str,
        credential: &str,
        chunk: &ToolOutputChunk,
        fingerprint: &str,
        ciphertext: &str,
        object: &str,
    ) -> Result<(), ApiError> {
        let _permit = self
            .object_writes
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| invalid())?;
        self.runtime_scope_access(id, credential, chrono::Utc::now().timestamp_millis())
            .await?;
        let mut tx = self.pool.begin().await?;
        let (row, _, _) = locked_workspace_with(&mut tx, id, WorkspaceLock::Artifact).await?;
        super::mailbox::runtime_auth(&row, credential, chrono::Utc::now().timestamp_millis())?;
        if !super::task_commands::command_mode(&row) {
            return Err(ApiError::conflict("object_write_runtime_fenced"));
        }
        let intent = super::object_writes::ObjectWriteIntent::prepare(
            &mut tx,
            &row,
            object,
            "tool",
            chrono::Utc::now().timestamp_millis(),
        )
        .await?;
        tx.commit().await?;
        let objects = self.transcript_objects.as_ref().ok_or_else(invalid)?;
        tokio::time::timeout(
            std::time::Duration::from_secs(30),
            objects.put(object, ciphertext.as_bytes()),
        )
        .await
        .map_err(|_| ApiError::unavailable("tool_output_upload_timeout"))??;
        self.runtime_scope_access(id, credential, chrono::Utc::now().timestamp_millis())
            .await?;
        let mut tx = self.pool.begin().await?;
        let (current, _, _) = locked_workspace_with(&mut tx, id, WorkspaceLock::Artifact).await?;
        super::mailbox::runtime_auth(&current, credential, chrono::Utc::now().timestamp_millis())?;
        intent.authorize_result(&current)?;
        let changed=sqlx::query("UPDATE zuse_rust_tool_output_chunks SET ready=true,ciphertext=NULL WHERE workspace_id=$1 AND blob_ref=$2 AND byte_offset=$3 AND fingerprint=$4 AND object_key=$5").bind(id).bind(&chunk.blob_ref).bind(chunk.offset as i64).bind(fingerprint).bind(object).execute(&mut *tx).await?;
        if changed.rows_affected() != 1 {
            return Err(ApiError::conflict("tool_output_immutable_conflict"));
        }
        intent.commit(&mut tx, &current).await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn tool_output_read(
        &self,
        access: &Access,
        id: &str,
        reference: &str,
        offset: usize,
        limit: usize,
    ) -> Result<Value, ApiError> {
        if !valid_ref(reference)
            || limit != CHUNK_BYTES
            || offset > zuse_cloud_runtime::tool_output::MAX_BYTES
        {
            return Err(ApiError::bad_request("invalid_tool_output_read"));
        }
        let mut tx = self.pool.begin().await?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
            .execute(&mut *tx)
            .await?;
        let row:Value=sqlx::query_scalar("SELECT to_jsonb(w) FROM api_cloud_workspaces w WHERE workspace_id=$1 AND account_id=$2")
            .bind(id).bind(&access.owner_id).fetch_optional(&mut *tx).await?.ok_or_else(||ApiError::not_found("cloud_workspace_not_found"))?;
        access.workspace_permission(&access.owner_id, &row["request_config"])?;
        if row["desired_state"] == "deleted"
            || matches!(row["state"].as_str(), Some("deleting" | "deleted"))
        {
            return Err(ApiError::not_found("cloud_workspace_not_found"));
        }
        let record=sqlx::query("SELECT c.ciphertext,c.object_key,c.next_offset,c.total_bytes FROM zuse_rust_tool_output_chunks c JOIN zuse_rust_tool_output_refs r USING(workspace_id,blob_ref) WHERE c.workspace_id=$1 AND c.blob_ref=$2 AND c.byte_offset=$3 AND c.ready")
            .bind(id).bind(reference).bind(offset as i64).fetch_optional(&mut *tx).await?.ok_or_else(||ApiError::not_found("tool_output_not_published"))?;
        tx.commit().await?;
        let ciphertext = if let Some(ciphertext) = record.get::<Option<String>, _>("ciphertext") {
            ciphertext
        } else {
            let object = record.get::<String, _>("object_key");
            let bytes = self
                .transcript_objects
                .as_ref()
                .ok_or_else(invalid)?
                .get(&object)
                .await?
                .ok_or_else(|| ApiError::not_found("tool_output_not_published"))?;
            String::from_utf8(bytes).map_err(|_| invalid())?
        };
        let plain = self
            .master
            .open_api(
                &context(&access.owner_id, id, reference, offset),
                &ciphertext,
            )
            .map_err(|_| invalid())?;
        let chunk: ToolOutputChunk = serde_json::from_slice(&plain).map_err(|_| invalid())?;
        if !chunk.valid()
            || chunk.blob_ref != reference
            || chunk.offset != offset
            || chunk.next_offset as i64 != record.get::<i64, _>("next_offset")
            || chunk.total_bytes as i64 != record.get::<i64, _>("total_bytes")
        {
            return Err(invalid());
        }
        let fresh = self
            .scope_access(id, Some(&access.actor.account_id), false)
            .await?;
        let effective = fresh.as_ref().map_or(access, |(access, _)| access);
        let current:Value=sqlx::query_scalar("SELECT to_jsonb(w) FROM api_cloud_workspaces w WHERE workspace_id=$1 AND account_id=$2").bind(id).bind(&access.owner_id).fetch_optional(&self.pool).await?.ok_or_else(||ApiError::not_found("cloud_workspace_not_found"))?;
        effective.workspace_permission(&access.owner_id, &current["request_config"])?;
        if matches!(current["state"].as_str(), Some("deleting" | "deleted"))
            || current["desired_state"] == "deleted"
            || fresh
                .as_ref()
                .is_some_and(|(_, policy)| current["request_config"]["sharingPolicy"] != *policy)
        {
            return Err(ApiError::not_found("cloud_workspace_not_found"));
        }
        Ok(
            json!({"text":chunk.text,"nextOffset":chunk.next_offset,"totalBytes":chunk.total_bytes,"done":chunk.next_offset==chunk.total_bytes}),
        )
    }
}
fn invalid() -> ApiError {
    ApiError::unavailable("cloud_tool_output_invalid")
}

#[cfg(test)]
mod live_tests;
