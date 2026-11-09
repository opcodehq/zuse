//! Read-only task history uses PostgreSQL and never dispatches execution or wakes compute.
use super::*;
use crate::{access::Access, transcript::Upload};
use sqlx::Row;

pub const MIGRATION: &str = include_str!("../../migrations/0016_task_history.sql");
const MAX_PAGE_BYTES: usize = 1024 * 1024;
const MAX_ENTRY_BYTES: usize = 512 * 1024;

async fn enabled(tx: &mut Transaction<'_, Postgres>) -> Result<bool, ApiError> {
    Ok(sqlx::query_scalar("SELECT to_regclass('zuse_rust_task_admissions') IS NOT NULL AND to_regclass('zuse_rust_task_history_messages') IS NOT NULL")
        .fetch_one(&mut **tx).await?)
}
fn admission_context(owner: &str, id: &str) -> String {
    format!("task-admission:{owner}:{id}")
}
fn history_context(owner: &str, id: &str, session: &str, sequence: Option<i64>) -> String {
    format!(
        "task-history:{owner}:{id}:{session}:{}",
        sequence
            .map(|s| s.to_string())
            .unwrap_or_else(|| "metadata".into())
    )
}

impl RuntimeControl {
    pub(super) async fn save_task_admission_tx(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        owner: &str,
        id: &str,
        session: &str,
        intent: &Value,
        now: i64,
    ) -> Result<(), ApiError> {
        if !enabled(tx).await? {
            return Ok(());
        }
        let ciphertext = self
            .master
            .seal_api(&admission_context(owner, id), &intent.to_string())
            .map_err(|_| ApiError::unavailable("task_admission_encryption_failed"))?;
        sqlx::query("INSERT INTO zuse_rust_task_admissions(workspace_id,session_id,ciphertext,created_at) VALUES($1,$2,$3,$4)")
            .bind(id).bind(session).bind(ciphertext).bind(now).execute(&mut **tx).await?;
        Ok(())
    }

    /// The caller has authenticated the generation and committed the exact native
    /// transcript cursor. Projection writes share that transaction and workspace lock.
    pub(super) async fn save_task_history_tx(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        row: &Value,
        upload: &Upload,
        now: i64,
    ) -> Result<(), ApiError> {
        if !enabled(tx).await?
            || upload.payload_format.as_deref() != Some(zuse_cloud_projection::FORMAT)
        {
            return Ok(());
        }
        let owner = row["account_id"].as_str().ok_or_else(invalid)?;
        let id = row["workspace_id"].as_str().ok_or_else(invalid)?;
        let wrapped = row["wrapped_transcript_key"].as_str().ok_or_else(invalid)?;
        let encoded = self
            .master
            .open_transcript_key(owner, id, wrapped)
            .map_err(|_| invalid())?;
        let key = zuse_cloud_projection::TranscriptKey::parse(&encoded).map_err(|_| invalid())?;
        let mut value: Value = key
            .open(
                id,
                &upload.session_id,
                &upload.cursor,
                upload.before_sequence,
                &upload.ciphertext,
            )
            .map_err(|_| invalid())?;
        if value["workspaceId"] != id
            || value["sessionId"] != upload.session_id
            || value["cursor"] != json!(upload.cursor)
            || value["payloadFormat"] != zuse_cloud_projection::FORMAT
        {
            return Err(invalid());
        }
        let generation = row["request_config"]["runtimeGeneration"]
            .as_i64()
            .ok_or_else(invalid)?;
        if value["runtimeGeneration"] != generation {
            return Err(invalid());
        }
        let mut head_exists = None;
        let (start, entries) = if let Some(before) = upload.before_sequence {
            serde_json::from_value::<zuse_cloud_projection::Page>(value.clone())
                .map_err(|_| invalid())?;
            let entries = value["entries"].as_array().ok_or_else(invalid)?.clone();
            if entries.len() > 64 {
                return Err(invalid());
            }
            let start = before
                .checked_sub(entries.len() as i64)
                .filter(|s| *s >= 0)
                .ok_or_else(invalid)?;
            if value["beforeSequence"] != before
                || value["olderMessageSequence"].as_i64().unwrap_or(0) != start
            {
                return Err(invalid());
            }
            (start, entries)
        } else {
            serde_json::from_value::<zuse_cloud_projection::Head>(value.clone())
                .map_err(|_| invalid())?;
            let entries = value["transcript"]["reset"]
                .as_array()
                .ok_or_else(invalid)?
                .clone();
            let start = value["olderMessageSequence"].as_i64().unwrap_or(0);
            if start < 0 || start > 1_000_000 || entries.len() > 64 {
                return Err(invalid());
            }
            value["transcript"]["reset"] = json!([]);
            value["transcript"]
                .as_object_mut()
                .ok_or_else(invalid)?
                .remove("replayBaseline");
            let metadata = self
                .master
                .seal_api(
                    &history_context(owner, id, &upload.session_id, None),
                    &value.to_string(),
                )
                .map_err(|_| invalid())?;
            if metadata.len() > 2 * 1024 * 1024 {
                return Ok(());
            } // The authenticated object projection remains the fallback.
            let total = start + entries.len() as i64;
            if std::env::var("ZUSE_CLOUD_ACCOUNT_EVENTS").as_deref() == Ok("true") {
                let last = entries.last();
                crate::events::store::projection_summary(
                    tx,
                    id,
                    &upload.session_id,
                    total,
                    last.and_then(|e| e["id"].as_str()),
                    last.and_then(|e| e["status"].as_str()),
                    now,
                )
                .await?;
            }
            head_exists = Some(sqlx::query_scalar::<_,bool>("WITH saved AS (INSERT INTO zuse_rust_task_history_sessions(workspace_id,session_id,runtime_generation,stream_epoch,stream_version,message_count,ciphertext,updated_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8) ON CONFLICT(workspace_id,session_id) DO UPDATE SET runtime_generation=EXCLUDED.runtime_generation,stream_epoch=EXCLUDED.stream_epoch,stream_version=EXCLUDED.stream_version,message_count=EXCLUDED.message_count,ciphertext=EXCLUDED.ciphertext,updated_at=EXCLUDED.updated_at RETURNING workspace_id,session_id,runtime_generation,stream_epoch,stream_version,message_count), trimmed AS (DELETE FROM zuse_rust_task_history_messages m USING saved s WHERE m.workspace_id=s.workspace_id AND m.session_id=s.session_id AND m.message_sequence>$6) SELECT EXISTS(SELECT 1 FROM saved WHERE workspace_id=$1 AND session_id=$2 AND runtime_generation=$3 AND stream_epoch=$4 AND stream_version=$5 AND message_count>=$6)")
                .bind(id).bind(&upload.session_id).bind(generation).bind(&upload.cursor.epoch).bind(upload.cursor.version).bind(total).bind(metadata).bind(now).fetch_one(&mut **tx).await?);
            (start, entries)
        };
        let exists: bool = if let Some(exists) = head_exists {
            exists
        } else {
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM zuse_rust_task_history_sessions WHERE workspace_id=$1 AND session_id=$2 AND runtime_generation=$3 AND stream_epoch=$4 AND stream_version=$5 AND message_count>=$6)")
                .bind(id).bind(&upload.session_id).bind(generation).bind(&upload.cursor.epoch).bind(upload.cursor.version).bind(start+entries.len() as i64).fetch_one(&mut **tx).await?
        };
        if !exists {
            return Ok(());
        }
        super::tool_output::allow_refs(tx, id, &upload.session_id, &entries).await?;
        let mut messages = Vec::with_capacity(entries.len());
        let mut oversized = Vec::new();
        let cursor = format!(
            "{generation}:{}:{}",
            upload.cursor.epoch, upload.cursor.version
        );
        for (index, entry) in entries.iter().enumerate() {
            let sequence = start + index as i64 + 1;
            let plain = entry.to_string();
            if plain.len() > MAX_ENTRY_BYTES {
                // Large payloads remain accessible through the original encrypted object contract.
                oversized.push(sequence);
                continue;
            }
            let ciphertext = self
                .master
                .seal_api(
                    &history_context(owner, id, &upload.session_id, Some(sequence)),
                    &plain,
                )
                .map_err(|_| invalid())?;
            messages.push(json!({"sequence":sequence,"ciphertext":ciphertext}));
        }
        if !messages.is_empty() || !oversized.is_empty() {
            // Sequences are unique and the two sets are disjoint. Publish the
            // whole bounded page in one round trip while retaining the caller's
            // authenticated cursor transaction and workspace lock.
            sqlx::query("WITH removed AS (DELETE FROM zuse_rust_task_history_messages WHERE workspace_id=$1 AND session_id=$2 AND message_sequence=ANY($5::bigint[])) INSERT INTO zuse_rust_task_history_messages(workspace_id,session_id,message_sequence,projection_cursor,ciphertext) SELECT $1,$2,m.sequence,$3,m.ciphertext FROM jsonb_to_recordset($4::jsonb) AS m(sequence bigint,ciphertext text) ON CONFLICT(workspace_id,session_id,message_sequence) DO UPDATE SET projection_cursor=EXCLUDED.projection_cursor,ciphertext=EXCLUDED.ciphertext")
                .bind(id).bind(&upload.session_id).bind(cursor).bind(json!(messages)).bind(oversized).execute(&mut **tx).await?;
        }
        Ok(())
    }

    pub async fn task_history(
        &self,
        access: &Access,
        id: &str,
        session: &str,
        before: Option<i64>,
    ) -> Result<Value, ApiError> {
        self.task_history_cached(access, id, session, before, false)
            .await
    }
    pub async fn task_history_cached(
        &self,
        access: &Access,
        id: &str,
        session: &str,
        before: Option<i64>,
        include_cache: bool,
    ) -> Result<Value, ApiError> {
        // Native org enrollment/sharing is checked before the read transaction;
        // legacy history retains its original membership path.
        let fresh = if access.membership.is_some() {
            let native:bool=sqlx::query_scalar("SELECT coalesce(bool_or(request_config->>'commandDeliveryMode'='server-admitted-v2'),false) FROM api_cloud_workspaces WHERE workspace_id=$1 AND account_id=$2").bind(id).bind(&access.owner_id).fetch_one(&self.pool).await?;
            if native {
                self.scope_access(id, Some(&access.actor.account_id), false)
                    .await?
            } else {
                None
            }
        } else {
            None
        };
        let access = fresh.as_ref().map_or(access, |(access, _)| access);
        let mut tx = self.pool.begin().await?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
            .execute(&mut *tx)
            .await?;
        let row: Value=sqlx::query_scalar("SELECT to_jsonb(w) FROM api_cloud_workspaces w WHERE workspace_id=$1 AND account_id=$2")
            .bind(id).bind(&access.owner_id).fetch_optional(&mut *tx).await?.ok_or_else(||ApiError::not_found("cloud_workspace_not_found"))?;
        access.workspace_permission(&access.owner_id, &row["request_config"])?;
        if fresh
            .as_ref()
            .is_some_and(|(_, policy)| row["request_config"]["sharingPolicy"] != *policy)
        {
            return Err(ApiError::conflict("workspace_sharing_changed"));
        }
        if matches!(row["state"].as_str(), Some("deleted" | "deleting"))
            || row["desired_state"] == "deleted"
        {
            return Err(ApiError::not_found("cloud_workspace_not_found"));
        }
        if !enabled(&mut tx).await? {
            return Err(ApiError::unavailable("cloud_task_history_schema_not_ready"));
        }
        let metadata=sqlx::query("SELECT ciphertext,message_count FROM zuse_rust_task_history_sessions WHERE workspace_id=$1 AND session_id=$2")
            .bind(id).bind(session).fetch_optional(&mut *tx).await?;
        // A runtime may publish its genuine empty head before checkout/auth are
        // ready. Keep the durable first user entry as a separate presentation
        // overlay; never alter that head's cursor, baseline or certificate.
        let mut admissions = self
            .task_command_admissions_tx(&mut tx, &row, session)
            .await?;
        if !super::task_commands::command_mode(&row)
            && before.is_none()
            && row["initial_session_id"] == session
            && metadata
                .as_ref()
                .is_some_and(|entry| entry.get::<i64, _>("message_count") == 0)
        {
            if let Some(entry)=sqlx::query("SELECT ciphertext,created_at FROM zuse_rust_task_admissions WHERE workspace_id=$1 AND session_id=$2").bind(id).bind(session).fetch_optional(&mut *tx).await? {
                let raw=self.master.open_api(&admission_context(&access.owner_id,id),entry.get("ciphertext")).map_err(|_|invalid())?;
                let intent:Value=serde_json::from_slice(&raw).map_err(|_|invalid())?;
                if intent["firstMessage"].as_str().is_some_and(|s|!s.trim().is_empty()) || intent["attachments"].as_array().is_some_and(|a|!a.is_empty()) {
                    let mid=intent["turnId"].as_str().filter(|s|!s.is_empty()).ok_or_else(invalid)?;
                    admissions.push(json!({"commandId":intent["commandId"],"messageId":mid,"sessionId":session,"text":intent["firstMessage"].as_str().unwrap_or_default(),"attachments":intent.get("attachments").cloned().unwrap_or(json!([])),"executionState":"accepted","createdAt":entry.get::<i64,_>("created_at")}));
                }
            }
        }
        let registered = self
            .task_registered_session_tx(&mut tx, &row, session)
            .await?;
        let conversations = self.task_registered_chats_tx(&mut tx, &row).await?;
        let queue_paused = registered.as_ref().and_then(|v| v["queuePaused"].as_bool());
        let decorate = |mut value: Value| {
            if !admissions.is_empty() {
                value["commandAdmissions"] = json!(admissions);
            }
            if super::task_commands::command_mode(&row) {
                value["commandDeliveryMode"] = json!(zuse_protocol::cloud_task_commands::MODE);
                value["queueRunSelectionVersion"] =
                    json!(zuse_protocol::cloud_task_commands::QUEUE_RUN_SELECTION_VERSION);
                value["commandAdmissionVersion"] =
                    json!(zuse_protocol::cloud_task_commands::VERSION);
                value["taskCommandCanEdit"] = json!(
                    access
                        .workspace_permission(&access.owner_id, &row["request_config"])
                        .ok()
                        == Some(crate::access::Permission::Edit)
                );
                value["registeredConversations"] = json!(conversations);
                value["queuePaused"] = json!(queue_paused);
            }
            value
        };

        if let Some(metadata) = metadata {
            let total: i64 = metadata.get("message_count");
            let before = before.unwrap_or(total + 1);
            if before < 1 || before > total + 1 {
                return Err(ApiError::bad_request("invalid_history_cursor"));
            }
            let plain = self
                .master
                .open_api(
                    &history_context(&access.owner_id, id, session, None),
                    metadata.get("ciphertext"),
                )
                .map_err(|_| invalid())?;
            let mut head: Value = serde_json::from_slice(&plain).map_err(|_| invalid())?;
            let cursor = format!(
                "{}:{}:{}",
                head["runtimeGeneration"].as_i64().ok_or_else(invalid)?,
                head["cursor"]["epoch"].as_str().ok_or_else(invalid)?,
                head["cursor"]["version"].as_i64().ok_or_else(invalid)?
            );
            let records=sqlx::query("SELECT message_sequence,ciphertext FROM zuse_rust_task_history_messages WHERE workspace_id=$1 AND session_id=$2 AND message_sequence<$3 AND projection_cursor=$4 ORDER BY message_sequence DESC LIMIT 64")
                .bind(id).bind(session).bind(before).bind(cursor).fetch_all(&mut *tx).await?;
            let mut next = before - 1;
            let mut bytes = 0;
            let mut entries = Vec::new();
            for record in records {
                let sequence: i64 = record.get("message_sequence");
                if sequence != next {
                    return Ok(
                        json!({"source":"object-fallback","reason":"history_page_not_in_postgres"}),
                    );
                }
                let ciphertext: &str = record.get("ciphertext");
                if !entries.is_empty() && bytes + ciphertext.len() > MAX_PAGE_BYTES {
                    break;
                }
                let plain = self
                    .master
                    .open_api(
                        &history_context(&access.owner_id, id, session, Some(sequence)),
                        ciphertext,
                    )
                    .map_err(|_| invalid())?;
                entries.push(serde_json::from_slice::<Value>(&plain).map_err(|_| invalid())?);
                bytes += ciphertext.len();
                next -= 1;
            }
            if entries.is_empty() && next > 0 {
                return Ok(
                    json!({"source":"object-fallback","reason":"history_page_not_in_postgres"}),
                );
            }
            entries.reverse();
            let older = (next > 0).then_some(next);
            head["transcript"]["reset"] = json!(entries);
            head["olderMessageSequence"] = json!(older);
            let projection_before = (before <= total).then_some(before - 1);
            let projection = if include_cache && projection_before != Some(0) {
                self.seal_history_cache(&row, &head, projection_before)
                    .unwrap_or(Value::Null)
            } else {
                Value::Null
            };
            return Ok(decorate(
                json!({"source":"runtime","head":head,"beforeSequence":before,"olderMessageSequence":older,"cacheProjection":projection}),
            ));
        }
        if before.is_some() {
            return Err(ApiError::bad_request("invalid_history_cursor"));
        }
        let admission=sqlx::query("SELECT session_id,ciphertext,created_at FROM zuse_rust_task_admissions WHERE workspace_id=$1 AND session_id=$2")
            .bind(id).bind(session).fetch_optional(&mut *tx).await?;
        if let Some(admission) = admission {
            let plain = self
                .master
                .open_api(
                    &admission_context(&access.owner_id, id),
                    admission.get("ciphertext"),
                )
                .map_err(|_| invalid())?;
            let intent: Value = serde_json::from_slice(&plain).map_err(|_| invalid())?;
            return Ok(decorate(
                json!({"source":"admission","workspaceId":id,"sessionId":session,"acceptedAt":admission.get::<i64,_>("created_at"),"admission":intent,"session":registered}),
            ));
        }
        if let Some(registered) = registered {
            return Ok(decorate(
                json!({"source":"admission","workspaceId":id,"sessionId":session,"session":registered}),
            ));
        }
        Ok(decorate(
            json!({"source":"object-fallback","reason":"legacy_history"}),
        ))
    }
    fn seal_history_cache(
        &self,
        row: &Value,
        head: &Value,
        before: Option<i64>,
    ) -> Result<Value, ApiError> {
        let id = row["workspace_id"].as_str().ok_or_else(invalid)?;
        let owner = row["account_id"].as_str().ok_or_else(invalid)?;
        let session = head["sessionId"].as_str().ok_or_else(invalid)?;
        let encoded = self
            .master
            .open_transcript_key(
                owner,
                id,
                row["wrapped_transcript_key"].as_str().ok_or_else(invalid)?,
            )
            .map_err(|_| invalid())?;
        let key = zuse_cloud_projection::TranscriptKey::parse(&encoded).map_err(|_| invalid())?;
        let cursor: zuse_cloud_projection::Cursor =
            serde_json::from_value(head["cursor"].clone()).map_err(|_| invalid())?;
        let payload = if let Some(before) = before {
            json!({"schemaVersion":1,"payloadFormat":zuse_cloud_projection::FORMAT,"workspaceId":id,"sessionId":session,"runtimeGeneration":head["runtimeGeneration"],"cursor":cursor,"beforeSequence":before,"entries":head["transcript"]["reset"],"olderMessageSequence":head["olderMessageSequence"]})
        } else {
            head.clone()
        };
        let upload = key
            .seal(id, session, &cursor, before, &payload)
            .map_err(|_| invalid())?;
        Ok(
            json!({"workspaceId":id,"sessionId":session,"cursor":cursor,"beforeSequence":before,"ciphertext":upload.ciphertext,"transcriptKey":encoded.as_str()}),
        )
    }
}
fn invalid() -> ApiError {
    ApiError::unavailable("cloud_task_history_invalid")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{access::Scope, auth::Principal};
    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};

    #[tokio::test]
    #[ignore = "requires explicit isolated localhost PostgreSQL"]
    async fn task_history_is_durable_scoped_paginated_and_bound_to_real_projection_cursors() {
        let url =
            std::env::var("ZUSE_CLOUD_TEST_DATABASE_URL").expect("isolated PostgreSQL required");
        let parsed = reqwest::Url::parse(&url).unwrap();
        assert!(matches!(
            parsed.host_str(),
            Some("localhost" | "127.0.0.1" | "::1")
        ));
        let admin = PgPool::connect(&url).await.unwrap();
        let schema = format!("task_history_{}", uuid::Uuid::new_v4().simple());
        sqlx::query(&format!("CREATE SCHEMA {schema}"))
            .execute(&admin)
            .await
            .unwrap();
        let scope = schema.clone();
        let pool = PgPoolOptions::new()
            .max_connections(3)
            .after_connect(move |connection, _| {
                let scope = scope.clone();
                Box::pin(async move {
                    sqlx::query(&format!("SET search_path={scope}"))
                        .execute(connection)
                        .await?;
                    Ok(())
                })
            })
            .connect(&url)
            .await
            .unwrap();
        sqlx::raw_sql("CREATE TABLE api_cloud_workspaces(workspace_id text PRIMARY KEY,account_id text,request_config jsonb,wrapped_transcript_key text,state text,desired_state text)").execute(&pool).await.unwrap();
        sqlx::raw_sql(MIGRATION).execute(&pool).await.unwrap();
        let master = MasterKey::parse(&URL_SAFE_NO_PAD.encode([26; 32])).unwrap();
        let (key, wrapped) = master.create_transcript_key("owner", "computer").unwrap();
        let signing = ed25519_dalek::SigningKey::from_bytes(&[27; 32]);
        let jwk = json!({"kty":"OKP","crv":"Ed25519","d":URL_SAFE_NO_PAD.encode([27;32]),"x":URL_SAFE_NO_PAD.encode(signing.verifying_key().as_bytes())});
        let control = RuntimeControl::from_pool(
            pool.clone(),
            master,
            TicketSigner::from_jwk(&jwk.to_string(), "https://api.example.test").unwrap(),
        );
        sqlx::query("INSERT INTO api_cloud_workspaces VALUES('computer','owner','{\"runtimeGeneration\":1}',$1,'setup','ready')").bind(wrapped).execute(&pool).await.unwrap();
        let access = Access {
            actor: Principal {
                account_id: "owner".into(),
            },
            scope: Scope::Personal,
            owner_id: "owner".into(),
            membership: None,
        };
        let mut tx = pool.begin().await.unwrap();
        control.save_task_admission_tx(&mut tx,"owner","computer","session",&json!({"commandId":"original-command","firstMessage":"Preserve this admitted prompt"}),10).await.unwrap();
        tx.commit().await.unwrap();
        let saved = control
            .task_history(&access, "computer", "session", None)
            .await
            .unwrap();
        assert_eq!(saved["source"], "admission");
        assert!(saved.get("cursor").is_none());
        assert_eq!(saved["admission"]["commandId"], "original-command");
        let stranger = Access {
            actor: Principal {
                account_id: "stranger".into(),
            },
            scope: Scope::Personal,
            owner_id: "stranger".into(),
            membership: None,
        };
        assert_eq!(
            control
                .task_history(&stranger, "computer", "session", None)
                .await
                .unwrap_err()
                .code,
            "cloud_workspace_not_found"
        );
        let entries:Vec<Value>=(0..70).map(|n| json!({"id":format!("message-{n}"),"role":"user","parts":[{"kind":"text","id":format!("part-{n}"),"text":format!("Saved message {n}")}],"createdAt":n,"deviceId":"guest"})).collect();
        let head:zuse_cloud_projection::Head=serde_json::from_value(json!({"schemaVersion":1,"payloadFormat":zuse_cloud_projection::FORMAT,"workspaceId":"computer","sessionId":"session","runtimeGeneration":1,"cursor":{"epoch":"real-engine-epoch","version":4},
            "chat":{"id":"session","deviceId":"guest","title":"Task","archived":false,"cwd":"/repo","branch":"task","checkoutId":null,"config":null,"lastMessagePreview":null,"lastMessageAt":null,"createdAt":"2026-10-30T00:00:00Z"},
            "conversations":[],"transcript":{"reset":entries},"queue":[],"queuePaused":false,"olderMessageSequence":null})).unwrap();
        let uploads = zuse_cloud_projection::encode(
            head,
            &zuse_cloud_projection::TranscriptKey::parse(&key).unwrap(),
        )
        .unwrap();
        let row: Value = sqlx::query_scalar(
            "SELECT to_jsonb(w) FROM api_cloud_workspaces w WHERE workspace_id='computer'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        for upload in &uploads {
            let mut tx = pool.begin().await.unwrap();
            control
                .save_task_history_tx(&mut tx, &row, upload, 20)
                .await
                .unwrap();
            tx.commit().await.unwrap();
        }
        let latest = control
            .task_history(&access, "computer", "session", None)
            .await
            .unwrap();
        assert_eq!(latest["source"], "runtime");
        assert_eq!(latest["head"]["cursor"]["epoch"], "real-engine-epoch");
        assert_eq!(latest["head"]["cursor"]["version"], 4);
        assert_eq!(
            latest["head"]["transcript"]["reset"]
                .as_array()
                .unwrap()
                .len(),
            64
        );
        assert_eq!(latest["olderMessageSequence"], 6);
        let cached = control
            .task_history_cached(&access, "computer", "session", None, true)
            .await
            .unwrap();
        let projection = &cached["cacheProjection"];
        let projection_key = zuse_cloud_projection::TranscriptKey::parse(
            projection["transcriptKey"].as_str().unwrap(),
        )
        .unwrap();
        let decoded: zuse_cloud_projection::Head = projection_key
            .open(
                "computer",
                "session",
                &serde_json::from_value(projection["cursor"].clone()).unwrap(),
                None,
                projection["ciphertext"].as_str().unwrap(),
            )
            .unwrap();
        assert_eq!(
            serde_json::to_value(decoded).unwrap()["transcript"]["reset"],
            cached["head"]["transcript"]["reset"]
        );
        let older = control
            .task_history(&access, "computer", "session", Some(7))
            .await
            .unwrap();
        assert_eq!(older["head"]["transcript"]["reset"][0]["id"], "message-0");
        assert!(older["olderMessageSequence"].is_null());
        let cached = control
            .task_history_cached(&access, "computer", "session", Some(7), true)
            .await
            .unwrap();
        let projection = &cached["cacheProjection"];
        assert_eq!(projection["beforeSequence"], 6);
        let decoded: zuse_cloud_projection::Page = projection_key
            .open(
                "computer",
                "session",
                &serde_json::from_value(projection["cursor"].clone()).unwrap(),
                Some(6),
                projection["ciphertext"].as_str().unwrap(),
            )
            .unwrap();
        assert_eq!(decoded.entries.len(), 6);
        assert_eq!(
            control
                .task_history(&access, "computer", "session", Some(999))
                .await
                .unwrap_err()
                .code,
            "invalid_history_cursor"
        );
        // A new head cannot silently label an earlier page as its own cursor.
        let mut next = uploads[0].clone();
        let projection_key = zuse_cloud_projection::TranscriptKey::parse(&key).unwrap();
        let mut value: Value = projection_key
            .open("computer", "session", &next.cursor, None, &next.ciphertext)
            .unwrap();
        next.cursor.version = 5;
        value["cursor"]["version"] = json!(5);
        next = projection_key
            .seal("computer", "session", &next.cursor, None, &value)
            .unwrap();
        let mut tx = pool.begin().await.unwrap();
        control
            .save_task_history_tx(&mut tx, &row, &next, 30)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        let stale = control
            .task_history(&access, "computer", "session", Some(7))
            .await
            .unwrap();
        assert_eq!(stale["source"], "object-fallback");
        let raw: Vec<String> =
            sqlx::query_scalar("SELECT ciphertext FROM zuse_rust_task_history_messages")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert!(raw.iter().all(|cipher| !cipher.contains("Saved message")));
        // Replacing one entry by an oversized value removes its old cached
        // ciphertext without dropping the other entries in the same batch.
        let mut oversized = value.clone();
        oversized["cursor"]["version"] = json!(6);
        oversized["transcript"]["reset"][0]["parts"][0]["text"] =
            json!("x".repeat(MAX_ENTRY_BYTES));
        let cursor = zuse_cloud_projection::Cursor {
            epoch: next.cursor.epoch.clone(),
            version: 6,
        };
        let mixed = projection_key
            .seal("computer", "session", &cursor, None, &oversized)
            .unwrap();
        let mut tx = pool.begin().await.unwrap();
        control
            .save_task_history_tx(&mut tx, &row, &mixed, 31)
            .await
            .unwrap();
        tx.rollback().await.unwrap();
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM zuse_rust_task_history_messages")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(
            count, 70,
            "rolling back a batch must retain the original messages"
        );
        let mut tx = pool.begin().await.unwrap();
        control
            .save_task_history_tx(&mut tx, &row, &mixed, 32)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        let sequences:Vec<i64>=sqlx::query_scalar("SELECT message_sequence FROM zuse_rust_task_history_messages ORDER BY message_sequence").fetch_all(&pool).await.unwrap();
        assert_eq!(sequences.len(), 69);
        assert!(
            !sequences.contains(&7),
            "the first entry of this head starts at sequence seven"
        );
        assert!(sequences.contains(&70));
        let references = json!([{"parts":[
            {"kind":"tool","resolved":true,"outputRef":"session/output","diffRef":"session/output"},
            {"kind":"tool","resolved":true,"outputRef":"session/diff","diffRef":"other/private"},
            {"kind":"tool","resolved":false,"outputRef":"session/pending"},
            {"kind":"text","resolved":true,"outputRef":"session/text"},
            {"kind":"tool","resolved":true,"outputRef":"session/../invalid"}]}]);
        for _ in 0..2 {
            let mut tx = pool.begin().await.unwrap();
            super::super::tool_output::allow_refs(
                &mut tx,
                "computer",
                "session",
                references.as_array().unwrap(),
            )
            .await
            .unwrap();
            tx.commit().await.unwrap();
        }
        let refs: Vec<String> =
            sqlx::query_scalar("SELECT blob_ref FROM zuse_rust_tool_output_refs ORDER BY blob_ref")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(refs, vec!["session/diff", "session/output"]);
        // Summary and outbox publication must remain deduplicated and atomic
        // when sent as a single SQL statement.
        let event_tables = crate::events::store::MIGRATION
            .split("CREATE FUNCTION zuse_rust_workspace_event_outbox")
            .next()
            .unwrap();
        sqlx::raw_sql(event_tables).execute(&pool).await.unwrap();
        for _ in 0..2 {
            let mut tx = pool.begin().await.unwrap();
            crate::events::store::projection_summary(
                &mut tx,
                "computer",
                "session",
                70,
                Some("message-69"),
                None,
                40,
            )
            .await
            .unwrap();
            tx.commit().await.unwrap();
        }
        let mut tx = pool.begin().await.unwrap();
        crate::events::store::projection_summary(
            &mut tx,
            "computer",
            "session",
            71,
            Some("message-70"),
            Some("running"),
            41,
        )
        .await
        .unwrap();
        tx.rollback().await.unwrap();
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM zuse_rust_account_event_outbox")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(
            count, 1,
            "duplicate and rolled-back summaries must not publish events"
        );
        let mut tx = pool.begin().await.unwrap();
        crate::events::store::projection_summary(
            &mut tx,
            "computer",
            "session",
            71,
            Some("message-70"),
            Some("running"),
            42,
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
        let events:Vec<Value>=sqlx::query_scalar("SELECT jsonb_build_object('owner',owner_id,'payload',payload) FROM zuse_rust_account_event_outbox ORDER BY outbox_id").fetch_all(&pool).await.unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(
            events[1],
            json!({"owner":"owner","payload":{"messageCount":71,"lastMessageId":"message-70","lastMessageStatus":"running"}})
        );
        sqlx::query(
            "UPDATE api_cloud_workspaces SET desired_state='deleted' WHERE workspace_id='computer'",
        )
        .execute(&pool)
        .await
        .unwrap();
        assert_eq!(
            control
                .task_history(&access, "computer", "session", None)
                .await
                .unwrap_err()
                .code,
            "cloud_workspace_not_found"
        );
        pool.close().await;
        sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
            .execute(&admin)
            .await
            .unwrap();
        admin.close().await;
    }
}
