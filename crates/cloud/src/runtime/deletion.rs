//! Explicit deletion revokes execution first and reports completion only after physical cleanup.
use super::*;
use crate::{
    jobs,
    providers::{Machine, ProviderError},
};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
pub const MIGRATION: &str = include_str!("../../migrations/0008_delete.sql");
pub const JOB_KIND: &str = "workspace-delete";
pub const TOMBSTONE_MS: i64 = 30 * 86400000;
fn failed() -> ApiError {
    ApiError::unavailable("cloud_workspace_delete_failed")
}
fn fenced() -> ApiError {
    ApiError::conflict("cloud_workspace_delete_fenced")
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Target {
    id: Option<String>,
    name: Option<String>,
    uncertain: bool,
    done: bool,
}
#[async_trait]
pub trait DeletionProvider: Send + Sync {
    async fn find(&self, id_or_name: &str) -> Result<Option<Machine>, ProviderError>;
    async fn destroy(&self, id: &str) -> Result<(), ProviderError>;
}
#[async_trait]
impl DeletionProvider for crate::providers::boxd::Boxd {
    async fn find(&self, id: &str) -> Result<Option<Machine>, ProviderError> {
        self.inspect(id).await
    }
    async fn destroy(&self, id: &str) -> Result<(), ProviderError> {
        self.destroy(id).await
    }
}
impl RuntimeControl {
    pub async fn delete_workspace(
        &self,
        id: &str,
        access: &crate::access::Access,
        command: Option<&str>,
        now: i64,
    ) -> Result<Value, ApiError> {
        if command.is_some_and(|s| s.is_empty() || s.len() > 128 || s.chars().any(char::is_control))
        {
            return Err(ApiError::bad_request("invalid_lifecycle_command_id"));
        }
        let mut tx = self.pool.begin().await?;
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
            .bind(format!("cloud-create:{}", access.owner_id))
            .execute(&mut *tx)
            .await?;
        sqlx::query("SELECT generation FROM zuse_rust_control_owners WHERE resource_kind='workspace' AND resource_id=$1 AND backend='rust' FOR UPDATE").bind(id).fetch_optional(&mut *tx).await?;
        let (row, generation, epoch) = locked_workspace(&mut tx, id).await?;
        if row["account_id"] != access.owner_id {
            return Err(ApiError::not_found("cloud_workspace_not_found"));
        }
        if access.membership.is_some()
            || access.owner_id != access.actor.account_id
            || access.workspace_permission(&access.owner_id, &row["request_config"])?
                != crate::access::Permission::Edit
        {
            return Err(ApiError::forbidden("workspace_access_denied"));
        }
        let ready: bool =
            sqlx::query_scalar("SELECT to_regclass('zuse_rust_deletions') IS NOT NULL")
                .fetch_one(&mut *tx)
                .await?;
        if !ready {
            return Err(ApiError::unavailable("cloud_delete_schema_not_ready"));
        }
        let prior: Option<String> = sqlx::query_scalar(
            "SELECT operation_id FROM zuse_rust_deletions WHERE workspace_id=$1",
        )
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?;
        if let Some(operation) = prior {
            sqlx::query("UPDATE zuse_rust_jobs SET state='pending',attempts=0,due_at=$3,updated_at=$3,last_error_code=NULL WHERE kind=$1 AND resource_id=$2 AND payload->>'operationId'=$4 AND state='failed'").bind(JOB_KIND).bind(id).bind(now).bind(operation).execute(&mut *tx).await?;
            tx.commit().await?;
            return Ok(crate::store::Workspace { row }.public());
        }
        if row["provider"] != "boxd"
            || row["request_config"]["providerConnectionId"].is_string()
            || row["request_config"]["runtimeRpcProtocol"] != zuse_protocol::cloud_gateway::RUST_RPC
        {
            return Err(ApiError::conflict("cloud_delete_runtime_update_required"));
        }
        if !self
            .transcript_objects
            .as_ref()
            .is_some_and(|o| o.supports_purge())
        {
            return Err(ApiError::unavailable("cloud_object_purge_unsupported"));
        }
        let next = generation.checked_add(1).ok_or_else(fenced)?;
        let next_epoch = epoch.checked_add(1).ok_or_else(fenced)?;
        let fence = mailbox::destruction_fence(&row)
            .checked_add(1)
            .filter(|f| *f <= 9007199254740991)
            .ok_or_else(fenced)?;
        let operation = uuid::Uuid::new_v4().to_string();
        let command = command
            .map(str::to_owned)
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        let digest = self
            .master
            .digest_api(&format!("delete-workspace:{id}"), &command);
        let idempotency = row["idempotency_key"]
            .as_str()
            .map(|key| self.delete_idempotency(&access.owner_id, key));
        sqlx::query("INSERT INTO zuse_rust_deletions(workspace_id,account_id,operation_id,command_digest,idempotency_digest,generation,phase,requested_at) VALUES($1,$2,$3,$4,$5,$6,'providers',$7)").bind(id).bind(&access.owner_id).bind(&operation).bind(digest).bind(idempotency).bind(next).bind(now).execute(&mut *tx).await?;
        sqlx::query("UPDATE zuse_rust_control_owners SET generation=$2,updated_at=$3 WHERE resource_kind='workspace' AND resource_id=$1 AND backend='rust'").bind(id).bind(next).bind(now).execute(&mut *tx).await?;
        let mut config = row["request_config"].clone();
        config["runtimeGeneration"] = json!(next);
        config["gatewayEpoch"] = json!(next_epoch);
        config["destructionFence"] = json!(fence);
        config["rustDeleteOperation"] = json!({"id":operation,"phase":"providers"});
        config["cloudMailboxLifecyclePending"] =
            json!({"action":"delete","destructionFence":fence});
        sqlx::query("UPDATE api_cloud_workspaces SET state='deleting',desired_state='deleted',runtime_state='offline',runtime_boot_token_hash=NULL,runtime_boot_token_expires_at=NULL,runtime_credential_hash=NULL,request_config=$2,status_code='delete-pending',revision=revision+1,updated_at=GREATEST(updated_at+1,$3) WHERE workspace_id=$1").bind(id).bind(config).bind(now).execute(&mut *tx).await?;
        sqlx::query("DELETE FROM zuse_rust_gateway_leases WHERE workspace_id=$1")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("UPDATE zuse_rust_jobs SET state='completed',payload='{}',lease_token=NULL,lease_expires_at=NULL,last_error_code='workspace_deleted',updated_at=$2 WHERE resource_id=$1 AND kind LIKE 'workspace-%'").bind(id).bind(now).execute(&mut *tx).await?;
        jobs::enqueue(
            &mut tx,
            &format!("delete:{operation}"),
            JOB_KIND,
            id,
            next,
            &json!({"operationId":operation}),
            now,
        )
        .await?;
        let (row, _, _) = locked_workspace(&mut tx, id).await?;
        tx.commit().await?;
        Ok(crate::store::Workspace { row }.public())
    }
    pub(super) fn delete_idempotency(&self, account: &str, key: &str) -> String {
        self.master
            .digest_api(&format!("deleted-workspace-idempotency:{account}"), key)
    }
    pub async fn delete_once(
        &self,
        provider: &dyn DeletionProvider,
        now: i64,
    ) -> Result<bool, ApiError> {
        let ready: bool =
            sqlx::query_scalar("SELECT to_regclass('zuse_rust_deletions') IS NOT NULL")
                .fetch_one(&self.pool)
                .await?;
        if !ready {
            return Ok(false);
        }
        let Some(job) = jobs::claim(&self.pool, JOB_KIND, "workspace", now, 120000).await? else {
            return Ok(false);
        };
        let result = self.delete_step(provider, &job, now).await;
        if let Err(error) = &result {
            jobs::retry(
                &self.pool,
                &job,
                chrono::Utc::now().timestamp_millis(),
                error.code,
            )
            .await?;
            sqlx::query("UPDATE api_cloud_workspaces SET status_code=$3,revision=revision+1 WHERE workspace_id=$1 AND desired_state='deleted' AND state='deleting' AND EXISTS(SELECT 1 FROM zuse_rust_control_owners WHERE resource_kind='workspace' AND resource_id=$1 AND backend='rust' AND generation=$2)").bind(&job.resource_id).bind(job.generation).bind(error.code).execute(&self.pool).await?;
        }
        result.map(|_| true)
    }
    async fn delete_step(
        &self,
        provider: &dyn DeletionProvider,
        job: &jobs::Job,
        now: i64,
    ) -> Result<(), ApiError> {
        let effects = match effects::Guard::acquire(&job.resource_id) {
            Ok(guard) => guard,
            Err(e) if e.code == "cloud_workspace_effects_busy" => {
                let mut tx = self.pool.begin().await?;
                let (row, generation, _) = locked_workspace(&mut tx, &job.resource_id).await?;
                recovery::active_job(&mut tx, job).await?;
                if generation != job.generation || row["desired_state"] != "deleted" {
                    return Err(fenced());
                }
                sqlx::query("UPDATE api_cloud_workspaces SET status_code='delete-waiting-for-execution' WHERE workspace_id=$1").bind(&job.resource_id).execute(&mut *tx).await?;
                recovery::defer(&mut tx, job, false, now).await?;
                tx.commit().await?;
                return Ok(());
            }
            Err(e) => return Err(e),
        };
        let mut tx = self.pool.begin().await?;
        let (row, generation, _) = locked_workspace(&mut tx, &job.resource_id).await?;
        recovery::active_job(&mut tx, job).await?;
        if generation != job.generation || row["desired_state"] != "deleted" {
            return Err(fenced());
        }
        let deletion:Value=sqlx::query_scalar("SELECT to_jsonb(d) FROM zuse_rust_deletions d WHERE workspace_id=$1 AND operation_id=$2 AND generation=$3 FOR UPDATE").bind(&job.resource_id).bind(job.payload["operationId"].as_str()).bind(job.generation).fetch_one(&mut *tx).await?;
        let complete = match deletion["phase"].as_str() {
            Some("providers") => {
                let mut targets: Vec<Target> =
                    serde_json::from_value(deletion["targets"].clone()).map_err(|_| fenced())?;
                if targets.is_empty() {
                    targets = collect(&mut tx, &row).await?;
                }
                if let Some(target) = targets.iter_mut().find(|t| !t.done) {
                    let identity = target
                        .id
                        .as_deref()
                        .or(target.name.as_deref())
                        .ok_or_else(fenced)?;
                    match provider.find(identity).await.map_err(|_| failed())? {
                        Some(machine) => {
                            if target.id.as_ref().is_some_and(|id| id != &machine.id)
                                || target
                                    .name
                                    .as_ref()
                                    .is_some_and(|name| name != &machine.name)
                            {
                                return Err(fenced());
                            }
                            let shared:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM api_cloud_workspaces WHERE workspace_id<>$1 AND provider=$2 AND provider_sandbox_id=$3 AND state<>'deleted')").bind(&job.resource_id).bind(required(&row,"provider")?).bind(&machine.id).fetch_one(&mut *tx).await?;
                            if shared {
                                return Err(ApiError::conflict("cloud_delete_source_shared"));
                            }
                            let persistent:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM api_machines m WHERE to_jsonb(m)->>'provider_sandbox_id'=$1 AND state NOT IN ('deleted','destroyed'))").bind(&machine.id).fetch_one(&mut *tx).await?;
                            if persistent {
                                return Err(ApiError::conflict("cloud_delete_source_shared"));
                            }
                            if target.id.is_none() {
                                // Persist the resolved exact ID before an irreversible delete or an uncertain reply.
                                target.id = Some(machine.id.clone());
                            } else {
                                provider.destroy(&machine.id).await.map_err(|_| failed())?;
                                target.done = provider
                                    .find(&machine.id)
                                    .await
                                    .map_err(|_| failed())?
                                    .is_none();
                            }
                        }
                        None if target.uncertain && target.id.is_none() => {
                            return Err(ApiError::unavailable(
                                "cloud_delete_allocation_outcome_unknown",
                            ));
                        }
                        None => target.done = true,
                    }
                }
                let done = targets.iter().all(|t| t.done);
                sqlx::query(
                    "UPDATE zuse_rust_deletions SET targets=$2,phase=$3 WHERE workspace_id=$1",
                )
                .bind(&job.resource_id)
                .bind(serde_json::to_value(targets).map_err(|_| failed())?)
                .bind(if done { "objects" } else { "providers" })
                .execute(&mut *tx)
                .await?;
                false
            }
            Some("objects") => {
                let objects = self.transcript_objects.as_ref().ok_or_else(failed)?;
                let prefix = format!(
                    "workspaces/{}/",
                    crate::objects::encode(&job.resource_id, false)
                );
                if objects.purge_prefix(&prefix).await? {
                    sqlx::query(
                        "UPDATE zuse_rust_deletions SET phase='metadata' WHERE workspace_id=$1",
                    )
                    .bind(&job.resource_id)
                    .execute(&mut *tx)
                    .await?;
                }
                false
            }
            Some("metadata") => {
                effects.purge()?;
                purge_metadata(&mut tx, &job.resource_id).await?;
                let config = json!({"runtimeGeneration":job.generation,"gatewayEpoch":row["request_config"]["gatewayEpoch"],"runtimeRpcProtocol":"zuse-rpc-v1","destructionFence":mailbox::destruction_fence(&row),"cloudMailboxLifecycleDelivered":{"action":"delete","destructionFence":mailbox::destruction_fence(&row)},"rustDeleteOperation":{"id":deletion["operation_id"],"phase":"deleted"}});
                sqlx::query("UPDATE api_cloud_workspaces SET state='deleted',desired_state='deleted',status_code='deleted',runtime_state='offline',provider_sandbox_id=NULL,runtime_boot_token_hash=NULL,runtime_boot_token_expires_at=NULL,runtime_credential_hash=NULL,wrapped_transcript_key=NULL,request_config=$2,branch='',base_ref='',archive_requested_at=NULL,archive_delete_at=NULL,deleted_at=$3,deletion_tombstone_expires_at=$4,revision=revision+1,updated_at=GREATEST(updated_at+1,$3),next_action_at=9007199254740991 WHERE workspace_id=$1").bind(&job.resource_id).bind(config).bind(now).bind(now+TOMBSTONE_MS).execute(&mut *tx).await?;
                if row.get("idempotency_key").is_some() {
                    sqlx::query(
                        "UPDATE api_cloud_workspaces SET idempotency_key=$2 WHERE workspace_id=$1",
                    )
                    .bind(&job.resource_id)
                    .bind(format!("deleted:{}", job.resource_id))
                    .execute(&mut *tx)
                    .await?;
                }
                sqlx::query("UPDATE zuse_rust_deletions SET phase='deleted',targets='[]',completed_at=$2,expires_at=$3 WHERE workspace_id=$1").bind(&job.resource_id).bind(now).bind(now+TOMBSTONE_MS).execute(&mut *tx).await?;
                true
            }
            Some("deleted") => true,
            _ => return Err(fenced()),
        };
        if !complete {
            let phase: String =
                sqlx::query_scalar("SELECT phase FROM zuse_rust_deletions WHERE workspace_id=$1")
                    .bind(&job.resource_id)
                    .fetch_one(&mut *tx)
                    .await?;
            let status = match phase.as_str() {
                "providers" => "delete-provider-cleanup",
                "objects" => "delete-purging-data",
                "metadata" => "delete-clearing-metadata",
                _ => return Err(fenced()),
            };
            sqlx::query("UPDATE api_cloud_workspaces SET status_code=$2,request_config=jsonb_set(request_config,'{rustDeleteOperation}',$3),revision=revision+1,updated_at=GREATEST(updated_at+1,$4) WHERE workspace_id=$1").bind(&job.resource_id).bind(status).bind(json!({"id":deletion["operation_id"],"phase":phase})).bind(now).execute(&mut *tx).await?;
        }
        recovery::defer(&mut tx, job, complete, now).await?;
        tx.commit().await?;
        Ok(())
    }
}
async fn collect(tx: &mut Transaction<'_, Postgres>, row: &Value) -> Result<Vec<Target>, ApiError> {
    let id = required(row, "workspace_id")?;
    let mut targets = Vec::<Target>::new();
    if let Some(source) = row["provider_sandbox_id"].as_str() {
        targets.push(Target {
            id: Some(source.into()),
            name: None,
            uncertain: false,
            done: false,
        });
    }
    let records:Vec<Value>=sqlx::query_scalar("SELECT jsonb_build_object('id',machine_id,'name',machine_name,'uncertain',false) FROM zuse_rust_allocations WHERE workspace_id=$1 UNION ALL SELECT jsonb_build_object('id',machine_id,'name',machine_name,'uncertain',false) FROM zuse_rust_restores WHERE workspace_id=$1 UNION ALL SELECT jsonb_build_object('id',source_machine_id,'name',NULL,'uncertain',false) FROM zuse_rust_recovery_operations WHERE workspace_id=$1 UNION ALL SELECT jsonb_build_object('id',machine_id,'name',machine_name,'uncertain',outcome IN ('pending','unknown')) FROM zuse_rust_provider_creates WHERE workspace_id=$1").bind(id).fetch_all(&mut **tx).await?;
    for record in records {
        let target = Target {
            id: record["id"].as_str().map(str::to_owned),
            name: record["name"].as_str().map(str::to_owned),
            uncertain: record["uncertain"] == true,
            done: false,
        };
        if target.id.is_none() && target.name.is_none() {
            continue;
        }
        if let Some(existing) = targets.iter_mut().find(|t| {
            target.id.is_some() && t.id == target.id
                || target.name.is_some() && t.name == target.name
        }) {
            existing.uncertain |= target.uncertain;
            if existing.name.is_none() {
                existing.name = target.name;
            }
            if existing.id.is_none() {
                existing.id = target.id;
            }
        } else {
            targets.push(target)
        }
    }
    Ok(targets)
}
async fn purge_metadata(tx: &mut Transaction<'_, Postgres>, id: &str) -> Result<(), ApiError> {
    // Names are a fixed schema inventory, never supplied by a caller. Financial usage/settlement is retained.
    for table in [
        "api_cloud_workspace_launch_intents",
        "api_cloud_transcript_checkpoints",
        "api_cloud_workspace_runtime_summaries",
        "api_cloud_workspace_api_messages",
        "api_cloud_workspace_api_turn_receipts",
        "api_cloud_workspace_command_receipts",
        "zuse_rust_mailbox_commands",
        "zuse_rust_mailboxes",
        "zuse_rust_restores",
        "zuse_rust_recovery_operations",
        "zuse_rust_allocations",
        "zuse_rust_lifecycle_operations",
        "zuse_rust_provider_creates",
        "zuse_rust_gateway_leases",
    ] {
        let exists: bool = sqlx::query_scalar("SELECT to_regclass($1) IS NOT NULL")
            .bind(table)
            .fetch_one(&mut **tx)
            .await?;
        if exists {
            sqlx::query(&format!("DELETE FROM {table} WHERE workspace_id=$1"))
                .bind(id)
                .execute(&mut **tx)
                .await?;
        }
    }
    sqlx::query(
        "DELETE FROM zuse_rust_jobs WHERE resource_id=$1 AND kind LIKE 'workspace-%' AND kind<>$2",
    )
    .bind(id)
    .bind(JOB_KIND)
    .execute(&mut **tx)
    .await?;
    Ok(())
}
