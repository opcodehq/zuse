//! Workspace-row serialized delivery. Lease expiry never authorizes tool replay.
use super::*;
use crate::access::Access;
use sqlx::Row;
use zuse_protocol::cloud_commands::{
    self as wire, CommandEnvelope, LeasePage, LeaseRequest, RuntimeAcknowledgment, RuntimeLease,
};

pub const MIGRATION: &str = include_str!("../../migrations/0003_command_mailbox.sql");
const TERMINAL: &str = "('applied','rejected','expired','cancelled','outcome-unknown')";

impl RuntimeControl {
    pub async fn command_enqueue(
        &self,
        id: &str,
        access: &Access,
        mut command: CommandEnvelope,
        now: i64,
    ) -> Result<Value, ApiError> {
        if command.workspace_id != id || !command.eligible() {
            return Err(ApiError::bad_request("cloud_command_not_eligible"));
        }
        let mut tx = self.pool.begin().await?;
        let (row, generation, _) = locked_workspace(&mut tx, id).await?;
        authorize(&row, access, true)?;
        if blocked(&row)
            || row["desired_state"] == "archived"
            || row["request_config"]["cloudMailboxLifecyclePending"]["action"] == "archive"
        {
            return Err(ApiError::conflict("cloud_workspace_unavailable"));
        }
        if row["request_config"]["runtimeRpcProtocol"] != zuse_protocol::cloud_gateway::RUST_RPC
            || row["request_config"]["rustCommandMailboxEnabled"] != true
        {
            return Err(ApiError::conflict("cloud_command_runtime_update_required"));
        }
        if command.destruction_fence != destruction_fence(&row) {
            return Err(ApiError::conflict("cloud_command_fence_stale"));
        }
        // Personal author is derived from authorization; never accept a spoofed organization actor.
        command.actor = None;
        mailbox(&mut tx, id).await?;
        if let Some(existing) = sqlx::query("SELECT command_id,fingerprint,state,workspace_sequence,revision,accepted_at FROM zuse_rust_mailbox_commands WHERE workspace_id=$1 AND command_id=$2")
            .bind(id).bind(&command.command_id).fetch_optional(&mut *tx).await? {
            if existing.get::<String,_>("fingerprint") != command.fingerprint {
                return Err(ApiError::conflict("command-fingerprint-mismatch"));
            }
            if matches!(existing.get::<String,_>("state").as_str(), "accepted"|"waiting-for-runtime") {
                enqueue_wake(&mut tx,id,generation,&command.command_id,now).await?;
            }
            let result = acceptance(&existing);
            tx.commit().await?;
            return Ok(result);
        }
        let (count,bytes):(i64,i64)=sqlx::query_as(&format!("SELECT COUNT(*),COALESCE(SUM(envelope_bytes),0)::bigint FROM zuse_rust_mailbox_commands WHERE workspace_id=$1 AND state NOT IN {TERMINAL}"))
            .bind(id).fetch_one(&mut *tx).await?;
        let envelope = serde_json::to_value(&command)
            .map_err(|_| ApiError::bad_request("cloud_command_not_eligible"))?;
        let size = serde_json::to_vec(&envelope)
            .map_err(|_| ApiError::bad_request("cloud_command_not_eligible"))?
            .len() as i64;
        if count >= wire::MAX_NONTERMINAL || bytes + size > wire::MAX_MAILBOX_BYTES {
            return Err(ApiError::conflict("cloud_mailbox_full"));
        }
        let (sequence,revision):(i64,i64)=sqlx::query_as("UPDATE zuse_rust_mailboxes SET sequence=sequence+1,revision=revision+1 WHERE workspace_id=$1 RETURNING sequence,revision")
            .bind(id).fetch_one(&mut *tx).await?;
        sqlx::query("INSERT INTO zuse_rust_mailbox_commands (workspace_id,command_id,workspace_sequence,revision,lane,fingerprint,envelope,envelope_bytes,state,accepted_at,updated_at) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,'accepted',$9,$9)")
            .bind(id).bind(&command.command_id).bind(sequence).bind(revision).bind(format!("session:{}",command.session_id)).bind(&command.fingerprint).bind(envelope).bind(size).bind(now).execute(&mut *tx).await?;
        enqueue_wake(&mut tx, id, generation, &command.command_id, now).await?;
        tx.commit().await?;
        let _ = self.mailbox_changes.send(());
        Ok(
            json!({"commandId":command.command_id,"workspaceSequence":sequence,"revision":revision,"acceptedAt":now,"state":"accepted"}),
        )
    }
    /// Reads and cancellation never contact the provider or resume compute.
    pub async fn command_status(
        &self,
        id: &str,
        access: &Access,
        command: &str,
        cancel: bool,
        now: i64,
    ) -> Result<Value, ApiError> {
        let mut tx = self.pool.begin().await?;
        let (row, _, _) = locked_workspace(&mut tx, id).await?;
        authorize(&row, access, cancel)?;
        mailbox(&mut tx, id).await?;
        let existing = status_row(&mut tx, id, command).await?;
        if cancel
            && !existing["everLeased"].as_bool().unwrap_or(true)
            && matches!(
                existing["state"].as_str(),
                Some("accepted" | "waiting-for-runtime" | "blocked")
            )
        {
            change(
                &mut tx,
                id,
                command,
                "state='cancelled',category='client-cancelled'",
                now,
            )
            .await?;
        }
        let status = status_row(&mut tx, id, command).await?;
        tx.commit().await?;
        if cancel {
            let _ = self.mailbox_changes.send(());
        }
        Ok(status)
    }
    pub async fn command_watch(
        &self,
        id: &str,
        access: &Access,
        after: i64,
    ) -> Result<Value, ApiError> {
        if !(0..=9_007_199_254_740_991).contains(&after) {
            return Err(ApiError::bad_request("invalid_mailbox_revision"));
        }
        let mut tx = self.pool.begin().await?;
        let (row, _, _) = locked_workspace(&mut tx, id).await?;
        authorize(&row, access, false)?;
        mailbox(&mut tx, id).await?;
        let revision: i64 =
            sqlx::query_scalar("SELECT revision FROM zuse_rust_mailboxes WHERE workspace_id=$1")
                .bind(id)
                .fetch_one(&mut *tx)
                .await?;
        let rows=sqlx::query("SELECT command_id,revision FROM zuse_rust_mailbox_commands WHERE workspace_id=$1 AND revision>$2 ORDER BY revision LIMIT 100")
            .bind(id).bind(after).fetch_all(&mut *tx).await?;
        let mut changes = Vec::new();
        let mut next = after;
        for row in rows {
            next = row.get("revision");
            changes.push(status_row(&mut tx, id, &row.get::<String, _>("command_id")).await?);
        }
        tx.commit().await?;
        Ok(
            json!({"changes":changes,"nextRevision":if next==after{revision}else{next},"resetRequired":after>revision}),
        )
    }
    pub async fn command_data_key(&self, id: &str, access: &Access) -> Result<Value, ApiError> {
        let mut tx = self.pool.begin().await?;
        let (mut row, _, _) = locked_workspace(&mut tx, id).await?;
        authorize(&row, access, true)?;
        if row["desired_state"] == "deleted"
            || matches!(row["state"].as_str(), Some("deleting" | "deleted"))
            || row["request_config"]["cloudMailboxLifecyclePending"]["action"] == "delete"
            || row["request_config"]["cloudMailboxLifecycleDelivered"]["action"] == "delete"
        {
            return Err(ApiError::conflict("cloud_workspace_unavailable"));
        }
        // Install before the first guest exists. The workspace lock prevents key rotation on retry.
        if row["wrapped_transcript_key"].is_null() {
            let (_, wrapped) = self
                .master
                .create_transcript_key(&access.owner_id, id)
                .map_err(|_| ApiError::unavailable("cloud_transcript_key_unavailable"))?;
            sqlx::query("UPDATE api_cloud_workspaces SET wrapped_transcript_key=$2,revision=revision+1,updated_at=GREATEST(updated_at+1,$3) WHERE workspace_id=$1")
                .bind(id).bind(&wrapped).bind(chrono::Utc::now().timestamp_millis()).execute(&mut *tx).await?;
            row["wrapped_transcript_key"] = json!(wrapped);
        }
        let wrapped = row["wrapped_transcript_key"]
            .as_str()
            .ok_or_else(|| ApiError::unavailable("cloud_transcript_key_unavailable"))?;
        let key = self
            .master
            .open_transcript_key(&access.owner_id, id, wrapped)
            .map_err(|_| ApiError::unavailable("cloud_transcript_key_unavailable"))?;
        let enabled = !blocked(&row)
            && row["desired_state"] != "archived"
            && row["request_config"]["runtimeRpcProtocol"]
                == zuse_protocol::cloud_gateway::RUST_RPC
            && row["request_config"]["rustCommandMailboxEnabled"] == true;
        tx.commit().await?;
        Ok(
            json!({"workspaceId":id,"encodedKey":key.as_str(),"keyVersion":1,"destructionFence":destruction_fence(&row),"mailboxEnabled":enabled}),
        )
    }
    pub async fn command_lease(
        &self,
        id: &str,
        credential: &str,
        request: &LeaseRequest,
        now: i64,
    ) -> Result<LeasePage, ApiError> {
        if request.storage_incarnation_id.is_empty()
            || request.storage_incarnation_id.len() > 512
            || request.wait_ms.is_some_and(|v| v > wire::WATCH_MAX_WAIT_MS)
            || request
                .after_revision
                .is_some_and(|v| !(0..=9_007_199_254_740_991).contains(&v))
        {
            return Err(ApiError::bad_request("cloud_mailbox_revision_invalid"));
        }
        let mut tx = self.pool.begin().await?;
        let (row, generation, _) = locked_workspace(&mut tx, id).await?;
        runtime_auth(&row, credential, now)?;
        if row["request_config"]["rustCommandMailboxEnabled"] != true {
            return Err(ApiError::conflict("cloud_command_mailbox_disabled"));
        }
        if blocked(&row)
            || row["desired_state"] != "ready"
            || row["state"] != "ready"
            || row["runtime_state"] != "online"
            || !storage_enrolled(&row)
            || row["request_config"]["cloudMailboxLifecyclePending"]["action"] == "archive"
        {
            return Err(ApiError::conflict("cloud_workspace_runtime_not_ready"));
        }
        if row["request_config"]["rustRuntimeStorage"]["storageIncarnationId"]
            != request.storage_incarnation_id
        {
            return Err(ApiError::conflict("runtime-storage-replaced"));
        }
        let sandbox = required(&row, "provider_sandbox_id")?;
        if row["request_config"]["rustStorageEnrollment"]["storageIncarnationId"]
            .as_str()
            .is_some_and(|v| v != request.storage_incarnation_id)
        {
            return Err(ApiError::conflict("runtime-storage-replaced"));
        }
        mailbox(&mut tx, id).await?;
        // Lifecycle fences invalidate envelopes accepted before a destructive operation.
        let stale=sqlx::query("SELECT command_id,ever_leased FROM zuse_rust_mailbox_commands WHERE workspace_id=$1 AND (envelope->>'destructionFence')::bigint<$2 AND state NOT IN ('applied','rejected','expired','cancelled','outcome-unknown')")
            .bind(id).bind(destruction_fence(&row)).fetch_all(&mut *tx).await?;
        for command in stale {
            let assignment = if command.get::<bool, _>("ever_leased") {
                "state='outcome-unknown',category='workspace-destruction-fence-advanced-after-lease',lease_token=NULL,lease_deadline=NULL"
            } else {
                "state='cancelled',category='workspace-destruction-fence-advanced',lease_token=NULL,lease_deadline=NULL"
            };
            change(
                &mut tx,
                id,
                &command.get::<String, _>("command_id"),
                assignment,
                now,
            )
            .await?;
        }
        // Changed storage makes all previously attempted work uncertain. Never blind-replay it.
        let uncertain:Vec<String>=sqlx::query_scalar(&format!("SELECT command_id FROM zuse_rust_mailbox_commands WHERE workspace_id=$1 AND ever_leased AND storage_incarnation_id<>$2 AND state NOT IN {TERMINAL}"))
            .bind(id).bind(&request.storage_incarnation_id).fetch_all(&mut *tx).await?;
        for command in uncertain {
            change(&mut tx,id,&command,"state='outcome-unknown',category='runtime-storage-replaced',lease_token=NULL,lease_deadline=NULL",now).await?;
        }
        let expired:Vec<String>=sqlx::query_scalar("SELECT command_id FROM zuse_rust_mailbox_commands WHERE workspace_id=$1 AND state='leased' AND lease_deadline<=$2")
            .bind(id).bind(now).fetch_all(&mut *tx).await?;
        for command in expired {
            change(
                &mut tx,
                id,
                &command,
                "state='lease-expired-awaiting-fence',category='runtime-lease-expired'",
                now,
            )
            .await?;
        }
        let fenced:Vec<String>=sqlx::query_scalar("SELECT command_id FROM zuse_rust_mailbox_commands WHERE workspace_id=$1 AND state='lease-expired-awaiting-fence' AND runtime_generation<$2 AND storage_incarnation_id=$3")
            .bind(id).bind(generation).bind(&request.storage_incarnation_id).fetch_all(&mut *tx).await?;
        for command in fenced {
            change(&mut tx,id,&command,"state='waiting-for-runtime',category='runtime-generation-fenced',lease_token=NULL,lease_deadline=NULL",now).await?;
        }
        let fence_required:bool=sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM zuse_rust_mailbox_commands WHERE workspace_id=$1 AND state='lease-expired-awaiting-fence')")
            .bind(id).fetch_one(&mut *tx).await?;
        let mut leases = Vec::new();
        if !fence_required {
            let candidates=sqlx::query(&format!("SELECT command_id,envelope,workspace_sequence FROM zuse_rust_mailbox_commands c WHERE workspace_id=$1 AND state IN ('accepted','waiting-for-runtime') AND NOT EXISTS (SELECT 1 FROM zuse_rust_mailbox_commands prior WHERE prior.workspace_id=c.workspace_id AND prior.lane=c.lane AND prior.workspace_sequence<c.workspace_sequence AND prior.state NOT IN {TERMINAL}) ORDER BY workspace_sequence LIMIT 16"))
                .bind(id).fetch_all(&mut *tx).await?;
            for candidate in candidates {
                let command_id: String = candidate.get("command_id");
                let token = uuid::Uuid::new_v4().to_string();
                let deadline = now + wire::LEASE_TTL_MS;
                let revision = next_revision(&mut tx, id).await?;
                sqlx::query("UPDATE zuse_rust_mailbox_commands SET state='leased',ever_leased=true,lease_token=$3,lease_deadline=$4,runtime_generation=$5,provider_sandbox_id=$6,storage_incarnation_id=$7,revision=$8,updated_at=$9 WHERE workspace_id=$1 AND command_id=$2")
                    .bind(id).bind(&command_id).bind(&token).bind(deadline).bind(generation).bind(sandbox).bind(&request.storage_incarnation_id).bind(revision).bind(now).execute(&mut *tx).await?;
                leases.push(RuntimeLease {
                    command: serde_json::from_value(candidate.get("envelope"))
                        .map_err(|_| ApiError::unavailable("cloud_mailbox_envelope_invalid"))?,
                    workspace_sequence: candidate.get("workspace_sequence"),
                    lease_token: token,
                    lease_deadline: deadline,
                    runtime_generation: generation,
                    provider_sandbox_id: sandbox.into(),
                    storage_incarnation_id: request.storage_incarnation_id.clone(),
                });
            }
        }
        let count:i64=sqlx::query_scalar(&format!("SELECT COUNT(*) FROM zuse_rust_mailbox_commands WHERE workspace_id=$1 AND state NOT IN {TERMINAL}"))
            .bind(id).fetch_one(&mut *tx).await?;
        let revision: i64 =
            sqlx::query_scalar("SELECT revision FROM zuse_rust_mailboxes WHERE workspace_id=$1")
                .bind(id)
                .fetch_one(&mut *tx)
                .await?;
        tx.commit().await?;
        if !leases.is_empty() {
            let _ = self.mailbox_changes.send(());
        }
        Ok(LeasePage {
            leases,
            nonterminal_count: count,
            mailbox_revision: revision,
            fence_required,
        })
    }
    pub async fn command_ack(
        &self,
        id: &str,
        credential: &str,
        ack: &RuntimeAcknowledgment,
        now: i64,
    ) -> Result<Value, ApiError> {
        if ack.result_iv.as_ref().map_or(0, |v| v.len())
            + ack.result_ciphertext.as_ref().map_or(0, |v| v.len())
            > wire::MAX_ENVELOPE_BYTES
            || ack.category.as_ref().is_some_and(|v| v.len() > 256)
        {
            return Err(ApiError::bad_request("command-result-too-large"));
        }
        let mut tx = self.pool.begin().await?;
        let (row, generation, _) = locked_workspace(&mut tx, id).await?;
        runtime_auth(&row, credential, now)?;
        mailbox(&mut tx, id).await?;
        let current = status_row(&mut tx, id, &ack.command_id).await?;
        if current["fingerprint"] != ack.fingerprint {
            return Err(ApiError::conflict("command-fingerprint-mismatch"));
        }
        if terminal(current["state"].as_str().unwrap_or("")) {
            tx.commit().await?;
            return Ok(current);
        }
        let lease=sqlx::query("SELECT lease_token,runtime_generation,provider_sandbox_id,storage_incarnation_id FROM zuse_rust_mailbox_commands WHERE workspace_id=$1 AND command_id=$2")
            .bind(id).bind(&ack.command_id).fetch_one(&mut *tx).await?;
        if !matches!(
            current["state"].as_str(),
            Some("leased" | "lease-expired-awaiting-fence")
        ) || lease.get::<Option<String>, _>("lease_token").as_deref() != Some(&ack.lease_token)
            || lease.get::<Option<i64>, _>("runtime_generation") != Some(generation)
            || row["request_config"]["rustRuntimeStorage"]["storageIncarnationId"]
                != serde_json::to_value(lease.get::<Option<String>, _>("storage_incarnation_id"))
                    .unwrap_or(Value::Null)
            || row["provider_sandbox_id"]
                != serde_json::to_value(lease.get::<Option<String>, _>("provider_sandbox_id"))
                    .unwrap_or(Value::Null)
        {
            return Err(ApiError::conflict("stale-runtime-lease"));
        }
        let revision = next_revision(&mut tx, id).await?;
        let state = serde_json::to_value(ack.state).expect("state");
        sqlx::query("UPDATE zuse_rust_mailbox_commands SET state=$3,category=$4,result_iv=$5,result_ciphertext=$6,lease_token=NULL,lease_deadline=NULL,revision=$7,updated_at=$8 WHERE workspace_id=$1 AND command_id=$2")
            .bind(id).bind(&ack.command_id).bind(state.as_str().expect("state string")).bind(&ack.category).bind(&ack.result_iv).bind(&ack.result_ciphertext).bind(revision).bind(now).execute(&mut *tx).await?;
        let result = status_row(&mut tx, id, &ack.command_id).await?;
        tx.commit().await?;
        let _ = self.mailbox_changes.send(());
        Ok(result)
    }
}
fn authorize(row: &Value, access: &Access, edit: bool) -> Result<(), ApiError> {
    if row["account_id"] != access.owner_id {
        return Err(ApiError::not_found("cloud_workspace_not_found"));
    }
    if access.membership.is_some() {
        return Err(ApiError::conflict("workspace_runtime_update_required"));
    }
    let permission = access.workspace_permission(&access.owner_id, &row["request_config"])?;
    if edit && permission != crate::access::Permission::Edit {
        return Err(ApiError::forbidden("workspace_access_denied"));
    }
    Ok(())
}
fn runtime_auth(row: &Value, credential: &str, now: i64) -> Result<(), ApiError> {
    if !enrollment_matches(row)
        || row["runtime_credential_hash"]
            .as_str()
            .is_none_or(|v| !equal(v, &hash(credential)))
        || row["request_config"]["runtimeCredentialExpiresAtMs"]
            .as_i64()
            .unwrap_or(0)
            <= now
        || row["request_config"]["runtimeRpcProtocol"] != zuse_protocol::cloud_gateway::RUST_RPC
        || row["desired_state"] == "deleted"
        || matches!(row["state"].as_str(), Some("deleted" | "deleting"))
        || row["request_config"]["cloudMailboxLifecyclePending"]["action"] == "delete"
        || row["request_config"]["cloudMailboxLifecycleDelivered"]["action"] == "delete"
    {
        return Err(ApiError::unauthorized("workspace_runtime_fenced"));
    }
    Ok(())
}
fn destruction_fence(row: &Value) -> i64 {
    let config = &row["request_config"];
    [
        config["destructionFence"].as_i64(),
        config["cloudMailboxLifecyclePending"]["destructionFence"].as_i64(),
        config["cloudMailboxLifecycleDelivered"]["destructionFence"].as_i64(),
    ]
    .into_iter()
    .flatten()
    .filter(|v| *v >= 0)
    .max()
    .unwrap_or(0)
}
fn terminal(state: &str) -> bool {
    matches!(
        state,
        "applied" | "rejected" | "expired" | "cancelled" | "outcome-unknown"
    )
}
fn acceptance(row: &sqlx::postgres::PgRow) -> Value {
    let state: String = row.get("state");
    json!({"commandId":row.get::<String,_>("command_id"),"workspaceSequence":row.get::<i64,_>("workspace_sequence"),"revision":row.get::<i64,_>("revision"),"acceptedAt":row.get::<i64,_>("accepted_at"),"state":if matches!(state.as_str(),"waiting-for-runtime"|"blocked"){state.as_str()}else{"accepted"}})
}
async fn enqueue_wake(
    tx: &mut Transaction<'_, Postgres>,
    id: &str,
    generation: i64,
    command: &str,
    now: i64,
) -> Result<(), ApiError> {
    let key = format!(
        "mailbox-wake:{}",
        hash(&json!([id, generation, command]).to_string())
    );
    crate::jobs::enqueue(
        tx,
        &key,
        "mailbox-wake",
        id,
        generation,
        &json!({"workspaceId":id,"commandId":command}),
        now,
    )
    .await?;
    Ok(())
}
async fn mailbox(tx: &mut Transaction<'_, Postgres>, id: &str) -> Result<(), ApiError> {
    sqlx::query("INSERT INTO zuse_rust_mailboxes(workspace_id) VALUES ($1) ON CONFLICT DO NOTHING")
        .bind(id)
        .execute(&mut **tx)
        .await?;
    Ok(())
}
async fn next_revision(tx: &mut Transaction<'_, Postgres>, id: &str) -> Result<i64, ApiError> {
    Ok(sqlx::query_scalar("UPDATE zuse_rust_mailboxes SET revision=revision+1 WHERE workspace_id=$1 RETURNING revision").bind(id).fetch_one(&mut **tx).await?)
}
async fn change(
    tx: &mut Transaction<'_, Postgres>,
    id: &str,
    command: &str,
    assignment: &str,
    now: i64,
) -> Result<(), ApiError> {
    let revision = next_revision(tx, id).await?;
    sqlx::query(&format!("UPDATE zuse_rust_mailbox_commands SET {assignment},revision=$3,updated_at=$4 WHERE workspace_id=$1 AND command_id=$2"))
        .bind(id).bind(command).bind(revision).bind(now).execute(&mut **tx).await?;
    Ok(())
}
async fn status_row(
    tx: &mut Transaction<'_, Postgres>,
    id: &str,
    command: &str,
) -> Result<Value, ApiError> {
    let row=sqlx::query("SELECT command_id,workspace_sequence,revision,fingerprint,state,ever_leased,updated_at,accepted_at,category,result_iv,result_ciphertext FROM zuse_rust_mailbox_commands WHERE workspace_id=$1 AND command_id=$2")
        .bind(id).bind(command).fetch_optional(&mut **tx).await?.ok_or_else(||ApiError::not_found("command-not-found"))?;
    let mut value = json!({"commandId":row.get::<String,_>("command_id"),"workspaceSequence":row.get::<i64,_>("workspace_sequence"),"revision":row.get::<i64,_>("revision"),"fingerprint":row.get::<String,_>("fingerprint"),"state":row.get::<String,_>("state"),"everLeased":row.get::<bool,_>("ever_leased"),"updatedAt":row.get::<i64,_>("updated_at"),"acceptedAt":row.get::<i64,_>("accepted_at")});
    for (column, key) in [
        ("category", "category"),
        ("result_iv", "resultIv"),
        ("result_ciphertext", "resultCiphertext"),
    ] {
        if let Some(v) = row.get::<Option<String>, _>(column) {
            value[key] = json!(v);
        }
    }
    Ok(value)
}
