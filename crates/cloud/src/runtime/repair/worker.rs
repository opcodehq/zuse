use super::*;
const RESUME_ACK: &str = "SELECT EXISTS(SELECT 1 FROM zuse_rust_lifecycle_operations WHERE operation_id=$1 AND workspace_id=$2 AND generation=$3 AND source_machine_id=$4 AND storage_incarnation_id=$5 AND action='resume' AND phase='ready')";

fn failed() -> ApiError {
    ApiError::unavailable("runtime_repair_supervisor_unavailable")
}
fn fenced() -> ApiError {
    ApiError::conflict("runtime_repair_fenced")
}
pub(super) fn summary(reply: &Value, op: &Value) -> Result<Value, ApiError> {
    if reply["version"] != 1
        || reply["operationId"] != op["operation_id"]
        || reply["workspaceId"] != op["workspace_id"]
        || reply["generation"] != op["generation"]
        || reply["storageIncarnationId"] != op["storage_incarnation_id"]
        || !matches!(
            reply["phase"].as_str(),
            Some("stopping" | "stopped" | "ready")
        )
        || !reply["graceful"].is_boolean()
    {
        return Err(ApiError::conflict("runtime_repair_receipt_rejected"));
    }
    let mut safe = json!({"version":1,"operationId":reply["operationId"],"workspaceId":reply["workspaceId"],"generation":reply["generation"],"storageIncarnationId":reply["storageIncarnationId"],"phase":reply["phase"],"graceful":reply["graceful"]});
    for key in ["originalChildPid", "childPid"] {
        if let Some(pid) = reply[key]
            .as_u64()
            .filter(|pid| *pid > 0 && *pid <= u32::MAX as u64)
        {
            safe[key] = json!(pid);
        }
    }
    if let Some(r) = reply.get("recovery") {
        let chats = r["interruptedChats"]
            .as_array()
            .filter(|v| v.len() <= 64)
            .ok_or_else(|| ApiError::conflict("runtime_repair_receipt_rejected"))?;
        if chats.iter().any(|c| {
            c.as_str()
                .is_none_or(|s| s.is_empty() || s.len() > 160 || s.chars().any(char::is_control))
        }) {
            return Err(ApiError::conflict("runtime_repair_receipt_rejected"));
        }
        let count = r["quarantinedCommandCount"]
            .as_u64()
            .ok_or_else(|| ApiError::conflict("runtime_repair_receipt_rejected"))?;
        let receipts = r["receipts"]
            .as_array()
            .filter(|r| r.len() <= 128)
            .ok_or_else(|| ApiError::conflict("runtime_repair_receipt_rejected"))?;
        let mut seen = std::collections::BTreeSet::new();
        let mut checked = vec![];
        for receipt in receipts {
            let cid = receipt["commandId"]
                .as_str()
                .filter(|s| !s.is_empty() && s.len() <= 160 && !s.chars().any(char::is_control))
                .ok_or_else(|| ApiError::conflict("runtime_repair_receipt_rejected"))?;
            if !seen.insert(cid)
                || !op["command_ids"]
                    .as_array()
                    .is_some_and(|ids| ids.iter().any(|id| id == cid))
            {
                return Err(ApiError::conflict("runtime_repair_receipt_rejected"));
            }
            if !receipt["fingerprint"].is_null()
                && receipt["fingerprint"]
                    .as_str()
                    .is_none_or(|s| s.is_empty() || s.len() > 128)
            {
                return Err(ApiError::conflict("runtime_repair_receipt_rejected"));
            }
            if !receipt["state"].is_null()
                && !matches!(
                    receipt["state"].as_str(),
                    Some("applied" | "rejected" | "expired" | "cancelled" | "outcome-unknown")
                )
            {
                return Err(ApiError::conflict("runtime_repair_receipt_rejected"));
            }
            checked.push(json!({"commandId":cid,"fingerprint":receipt["fingerprint"],"state":receipt["state"]}));
        }
        safe["recovery"] = json!({"interruptedChats":chats,"interruptedChatCount":r["interruptedChatCount"].as_u64().unwrap_or(chats.len()as u64),"interruptedChatsTruncated":r["interruptedChatsTruncated"]==true,"quarantinedCommandCount":count,"receipts":checked});
    }
    Ok(safe)
}
impl RuntimeControl {
    async fn repair_locked(
        &self,
        job: &jobs::Job,
    ) -> Result<(Transaction<'static, Postgres>, Value, Value), ApiError> {
        let mut tx = self.pool.begin().await?;
        let (row, generation, _) = locked_workspace(&mut tx, &job.resource_id).await?;
        let active:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM zuse_rust_jobs WHERE job_id=$1 AND lease_token=$2 AND state='running' AND lease_expires_at>$3 FOR UPDATE)").bind(&job.id).bind(&job.lease_token).bind(chrono::Utc::now().timestamp_millis()).fetch_one(&mut *tx).await?;
        let op:Value=sqlx::query_scalar("SELECT to_jsonb(r) FROM zuse_rust_runtime_repairs r WHERE operation_id=$1 AND workspace_id=$2 FOR UPDATE").bind(job.payload["operationId"].as_str()).bind(&job.resource_id).fetch_optional(&mut *tx).await?.ok_or_else(fenced)?;
        if !active
            || generation != job.generation
            || op["generation"] != generation
            || blocked(&row)
            || row["provider_sandbox_id"] != op["source_machine_id"]
            || row["request_config"]["rustRuntimeStorage"]["storageIncarnationId"]
                != op["storage_incarnation_id"]
            || row["request_config"]["rustRuntimeRepair"]["operationId"] != op["operation_id"]
        {
            return Err(fenced());
        }
        Ok((tx, row, op))
    }
    async fn repair_save(
        &self,
        job: &jobs::Job,
        expected: &str,
        next: &str,
        receipt: Option<&Value>,
        done: bool,
        now: i64,
    ) -> Result<(), ApiError> {
        let (mut tx, row, op) = self.repair_locked(job).await?;
        let now = now.max(op["updated_at"].as_i64().unwrap_or(0).saturating_add(1));
        if op["phase"] != expected {
            return Err(fenced());
        }
        if expected == "waiting-for-runtime" && done {
            let ack: bool = sqlx::query_scalar(RESUME_ACK)
                .bind(op["resume_operation_id"].as_str())
                .bind(&job.resource_id)
                .bind(job.generation)
                .bind(op["source_machine_id"].as_str())
                .bind(op["storage_incarnation_id"].as_str())
                .fetch_one(&mut *tx)
                .await?;
            // Check again under the workspace lock: a concurrent Sleep/Delete
            // must not turn an earlier online snapshot into completion proof.
            if !ack
                || row["state"] != "ready"
                || row["runtime_state"] != "online"
                || !storage_enrolled(&row)
            {
                return Err(fenced());
            }
        }
        let mut recovery = op["recovery"].clone();
        if let Some(receipt) = receipt {
            if let Some(data) = receipt.get("recovery") {
                let unknown = recovery.get("unknownServerCommandCount").cloned();
                recovery = data.clone();
                if let Some(unknown) = unknown {
                    recovery["unknownServerCommandCount"] = unknown;
                }
            }
            let column = if expected == "starting" {
                "start_receipt"
            } else {
                "stop_receipt"
            };
            sqlx::query(&format!(
                "UPDATE zuse_rust_runtime_repairs SET {column}=$2 WHERE operation_id=$1"
            ))
            .bind(op["operation_id"].as_str())
            .bind(receipt)
            .execute(&mut *tx)
            .await?;
        }
        let mut config = row["request_config"].clone();
        if expected == "waiting-for-runtime"
            && done
            && config["rustMemoryTransportRecovery"]["repairOperationId"] == op["operation_id"]
            && super::super::lifecycle::valid_memory_recovery(&row)
        {
            // Only this repair's bound Ready ACK and fresh storage enrollment
            // passed above. A healthy API, old Resumed or another operation
            // cannot clear the retained native transport fence.
            config
                .as_object_mut()
                .unwrap()
                .remove("rustMemoryTransportRecovery");
        }
        if expected == "starting" {
            // Only a disk-bound ACK with the original exact fingerprint can resolve an
            // uncertain attempt. A missing receipt never licenses resending that action.
            if let Some(receipts) = recovery["receipts"].as_array() {
                for local in receipts {
                    let cid = required(local, "commandId")?;
                    let Some(ack) = local["fingerprint"].as_str() else {
                        continue;
                    };
                    let Some(state) = local["state"]
                        .as_str()
                        .filter(|s| matches!(*s, "applied" | "rejected" | "expired" | "cancelled"))
                    else {
                        continue;
                    };
                    let command:Option<(String,String)>=sqlx::query_as("SELECT fingerprint,state FROM zuse_rust_mailbox_commands WHERE workspace_id=$1 AND command_id=$2 AND category='runtime-repair-interrupted' FOR UPDATE").bind(&job.resource_id).bind(cid).fetch_optional(&mut *tx).await?;
                    if let Some((fingerprint, current)) = command {
                        if !equal(ack, &fingerprint) {
                            return Err(ApiError::conflict(
                                "runtime_repair_receipt_fingerprint_mismatch",
                            ));
                        }
                        if current == "outcome-unknown" {
                            super::super::mailbox::change(&mut tx,&job.resource_id,cid,&format!("state='{state}',category='runtime-repair-local-ack',lease_token=NULL,lease_deadline=NULL"),now).await?;
                        }
                    }
                }
            }
            let unknown:i64=sqlx::query_scalar("SELECT count(*) FROM zuse_rust_mailbox_commands WHERE workspace_id=$1 AND command_id IN (SELECT jsonb_array_elements_text($2)) AND state='outcome-unknown'").bind(&job.resource_id).bind(&op["command_ids"]).fetch_one(&mut *tx).await?;
            recovery["unknownServerCommandCount"] = json!(unknown);
        }
        if next == "starting" {
            let attempted:Vec<(String,String)>=sqlx::query_as("SELECT command_id,fingerprint FROM zuse_rust_mailbox_commands WHERE workspace_id=$1 AND ever_leased AND state NOT IN ('applied','rejected','expired','cancelled','outcome-unknown') FOR UPDATE").bind(&job.resource_id).fetch_all(&mut *tx).await?;
            if attempted.len() > 128 {
                return Err(ApiError::conflict(
                    "runtime_repair_command_inventory_changed",
                ));
            }
            for (cid, fingerprint) in &attempted {
                if !op["command_ids"]
                    .as_array()
                    .is_some_and(|ids| ids.iter().any(|id| id == cid))
                {
                    return Err(ApiError::conflict(
                        "runtime_repair_command_inventory_changed",
                    ));
                }
                if let Some(local) = recovery["receipts"]
                    .as_array()
                    .and_then(|rs| rs.iter().find(|r| r["commandId"] == *cid))
                {
                    if local["fingerprint"]
                        .as_str()
                        .is_some_and(|f| !equal(f, &fingerprint))
                    {
                        return Err(ApiError::conflict(
                            "runtime_repair_receipt_fingerprint_mismatch",
                        ));
                    }
                }
                super::super::mailbox::change(&mut tx,&job.resource_id,cid,"state='outcome-unknown',category='runtime-repair-interrupted',lease_token=NULL,lease_deadline=NULL",now).await?;
            }
            recovery["unknownServerCommandCount"] = json!(attempted.len());
            sqlx::query("INSERT INTO zuse_rust_lifecycle_operations(operation_id,workspace_id,command_id,generation,source_machine_id,storage_incarnation_id,action,phase,created_at,updated_at) VALUES($1,$2,$3,$4,$5,$6,'resume','launching',$7,$7) ON CONFLICT(operation_id) DO NOTHING").bind(op["resume_operation_id"].as_str()).bind(&job.resource_id).bind(format!("repair-resume:{}",op["operation_id"].as_str().unwrap())).bind(job.generation).bind(op["source_machine_id"].as_str()).bind(op["storage_incarnation_id"].as_str()).bind(now).execute(&mut *tx).await?;
            config["rustLifecycleOperation"] =
                json!({"id":op["resume_operation_id"],"action":"resume","phase":"launching"});
            if let Some(object) = config.as_object_mut() {
                object.remove("rustMemoryParked");
            }
        }
        config["rustRuntimeRepair"]["phase"] = json!(next);
        config["rustRuntimeRepair"]["updatedAt"] = json!(now);
        config["rustRuntimeRepair"]["errorCode"] = Value::Null;
        sqlx::query("UPDATE zuse_rust_runtime_repairs SET phase=$2,recovery=$3,error_code=NULL,updated_at=$4 WHERE operation_id=$1").bind(op["operation_id"].as_str()).bind(next).bind(recovery).bind(now).execute(&mut *tx).await?;
        sqlx::query("UPDATE api_cloud_workspaces SET state=CASE WHEN $3='starting' THEN 'provisioning' ELSE state END,desired_state=CASE WHEN $3='starting' THEN 'ready' ELSE desired_state END,status_code=$4,request_config=$2,revision=revision+1,updated_at=GREATEST(updated_at+1,$5) WHERE workspace_id=$1").bind(&job.resource_id).bind(config).bind(next).bind(format!("runtime-repair-{next}")).bind(now).execute(&mut *tx).await?;
        let changed=sqlx::query("UPDATE zuse_rust_jobs SET state=CASE WHEN $3 THEN 'completed' ELSE 'pending' END,due_at=$4+500,attempts=GREATEST(attempts-1,0),lease_token=NULL,lease_expires_at=NULL,updated_at=$4 WHERE job_id=$1 AND lease_token=$2 AND state='running' AND lease_expires_at>$4").bind(&job.id).bind(&job.lease_token).bind(done).bind(now).execute(&mut *tx).await?;
        if changed.rows_affected() != 1 {
            return Err(fenced());
        }
        tx.commit().await?;
        let _ = self.mailbox_changes.send(());
        Ok(())
    }
    async fn repair_step(
        &self,
        provider: &dyn RepairProvider,
        job: &jobs::Job,
        now: i64,
    ) -> Result<(), ApiError> {
        let _effects = effects::Guard::acquire(&job.resource_id)?;
        let (tx, row, op) = self.repair_locked(job).await?;
        tx.commit().await?;
        if row["provider"] != provider.provider_id() {
            return Err(ApiError::conflict("cloud_provider_identity_mismatch"));
        }
        self.worker_job_scope_access(job, "repair", now).await?;
        if matches!(op["phase"].as_str(), Some("ready" | "attention-required")) {
            jobs::finish(&self.pool, job, "workspace", now).await?;
            return Ok(());
        }
        if now - op["run_started_at"].as_i64().ok_or_else(fenced)? > 180000 {
            return Err(ApiError::unavailable("runtime_repair_readiness_timeout"));
        }
        let phase = op["phase"].as_str().ok_or_else(fenced)?;
        if phase == "quiescing" {
            let quiesced = row["request_config"]["rustLifecycleOperation"]["id"]
                == op["pause_operation_id"]
                && row["request_config"]["rustLifecycleOperation"]["phase"] == "quiesced";
            return self
                .repair_save(
                    job,
                    phase,
                    if quiesced || now - op["created_at"].as_i64().unwrap_or(now) > 5000 {
                        "stopping"
                    } else {
                        phase
                    },
                    None,
                    false,
                    now,
                )
                .await;
        }
        if phase == "waiting-for-runtime" {
            if row["runtime_state"] == "online"
                && row["state"] == "ready"
                && sqlx::query_scalar::<_, bool>(RESUME_ACK)
                    .bind(op["resume_operation_id"].as_str())
                    .bind(&job.resource_id)
                    .bind(job.generation)
                    .bind(op["source_machine_id"].as_str())
                    .bind(op["storage_incarnation_id"].as_str())
                    .fetch_one(&self.pool)
                    .await?
                && storage_enrolled(&row)
            {
                let attention = op["recovery"]["interruptedChatCount"].as_u64().unwrap_or(0) > 0
                    || op["recovery"]["unknownServerCommandCount"]
                        .as_u64()
                        .unwrap_or(0)
                        > 0;
                return self
                    .repair_save(
                        job,
                        phase,
                        if attention {
                            "attention-required"
                        } else {
                            "ready"
                        },
                        None,
                        true,
                        now,
                    )
                    .await;
            }
            return self.repair_save(job, phase, phase, None, false, now).await;
        }
        let machine = provider
            .inspect(required(&op, "source_machine_id")?)
            .await
            .map_err(|_| failed())?
            .ok_or_else(|| ApiError::conflict("runtime_repair_computer_missing"))?;
        if machine.id != op["source_machine_id"] || machine.state != MachineState::Running {
            return Err(ApiError::conflict(
                "runtime_repair_requires_running_computer",
            ));
        }
        let action = if phase == "stopping" {
            "repair-stop"
        } else if phase == "stopped" {
            "repair-status"
        } else if phase == "starting" {
            "repair-start"
        } else {
            return Err(fenced());
        };
        let mut request = json!({"version":1,"action":action,"operationId":op["operation_id"],"workspaceId":job.resource_id,"generation":job.generation,"storageIncarnationId":op["storage_incarnation_id"],"apiUrl":self.signer.issuer()});
        if action == "repair-start" {
            request["resumeOperationId"] = op["resume_operation_id"].clone();
            request["commandIds"] = op["command_ids"].clone();
        }
        let receipt = summary(
            &provider
                .rpc(&machine.id, &request)
                .await
                .map_err(|_| failed())?,
            &op,
        )?;
        let next = match (phase, receipt["phase"].as_str()) {
            ("stopping", Some("stopping")) => "stopping",
            ("stopping", Some("stopped")) => "stopped",
            ("stopped", Some("stopped")) => "starting",
            ("starting", Some("ready")) => "waiting-for-runtime",
            ("starting", Some("stopped")) => "starting",
            _ => return Err(ApiError::conflict("runtime_repair_receipt_rejected")),
        };
        self.repair_save(job, phase, next, Some(&receipt), false, now)
            .await
    }
    pub async fn runtime_repair_once(
        &self,
        provider: &dyn RepairProvider,
        now: i64,
    ) -> Result<bool, ApiError> {
        let Some(job) = jobs::claim(&self.pool, JOB_KIND, "workspace", now, 120000).await? else {
            return Ok(false);
        };
        self.repair_claimed(provider, &job, now).await?;
        Ok(true)
    }
    pub async fn runtime_repair_registry_once(
        &self,
        registry: &crate::providers::registry::Registry,
        now: i64,
    ) -> Result<bool, ApiError> {
        let Some(job) = jobs::claim(&self.pool, JOB_KIND, "workspace", now, 120000).await? else {
            return Ok(false);
        };
        let provider = self.provider_for_job(registry, &job).await?;
        self.repair_claimed(&provider, &job, now).await?;
        Ok(true)
    }
    async fn repair_claimed(
        &self,
        provider: &dyn RepairProvider,
        job: &jobs::Job,
        now: i64,
    ) -> Result<(), ApiError> {
        let result = self.repair_step(provider, job, now).await;
        if let Err(error) = &result {
            // Fence both error metadata and retry scheduling together. A stale worker
            // cannot overwrite a newer successful supervisor receipt after losing its lease.
            if let Ok((mut tx, row, op)) = self.repair_locked(job).await {
                let at = chrono::Utc::now()
                    .timestamp_millis()
                    .max(op["updated_at"].as_i64().unwrap_or(0).saturating_add(1));
                let changed=sqlx::query("UPDATE zuse_rust_jobs SET state=CASE WHEN attempts>=max_attempts THEN 'failed' ELSE 'pending' END,due_at=$3+2000,lease_token=NULL,lease_expires_at=NULL,last_error_code=$4,updated_at=$3 WHERE job_id=$1 AND lease_token=$2 AND state='running' AND lease_expires_at>$3").bind(&job.id).bind(&job.lease_token).bind(at).bind(error.code).execute(&mut *tx).await?;
                if changed.rows_affected() == 1 {
                    sqlx::query("UPDATE zuse_rust_runtime_repairs SET error_code=$2,updated_at=GREATEST(updated_at+1,$3) WHERE operation_id=$1 AND phase NOT IN ('ready','attention-required')").bind(op["operation_id"].as_str()).bind(error.code).bind(at).execute(&mut *tx).await?;
                    let mut config = row["request_config"].clone();
                    config["rustRuntimeRepair"]["errorCode"] = json!(error.code);
                    config["rustRuntimeRepair"]["updatedAt"] = json!(at);
                    sqlx::query("UPDATE api_cloud_workspaces SET request_config=$2,revision=revision+1,updated_at=GREATEST(updated_at+1,$3) WHERE workspace_id=$1").bind(&job.resource_id).bind(config).bind(at).execute(&mut *tx).await?;
                    tx.commit().await?;
                }
            }
        }
        result
    }
}
