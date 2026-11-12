//! Original archive capabilities with durable native verification and retirement barriers.
use super::*;
use crate::providers::contract::{DesiredState, OperationState, ProviderOperation};
use crate::{
    jobs,
    objects::Objects,
    providers::{Machine, MachineState, ProviderError},
};
use async_trait::async_trait;
mod retirement;
#[cfg(test)]
mod retirement_tests;
mod stages;
pub(in crate::runtime) use stages::{
    EFFECT_MAX_SUBMISSIONS, EFFECT_SETTLE_MS, EFFECT_WAIT_LIMIT_MS,
};
#[cfg(test)]
mod stages_tests;
use zeroize::Zeroizing;
use zuse_cloud_runtime::recovery::{Binding, RustStorage};
use zuse_protocol::cloud::{ArchiveManifest, ArchivePhase, RecoveryArchive};
pub const MIGRATION: &str = include_str!("../../migrations/0006_recovery.sql");
pub const CAPABILITY: &str = "rust-full-state-recovery-v1";
pub const JOB_KIND: &str = "workspace-recovery";
pub const WIRE_LIMIT: usize = 6 * 1024 * 1024;
const TTL: i64 = 60 * 60 * 1000;
const LEASE: i64 = 120000;
fn failed() -> ApiError {
    ApiError::unavailable("cloud_archive_operation_failed")
}
fn fenced() -> ApiError {
    ApiError::conflict("cloud_archive_operation_fenced")
}
fn deleted(row: &Value) -> bool {
    lifecycle::deleted(row)
}
fn valid_object(name: &str) -> bool {
    name == "manifest"
        || name.len() == 64
            && name
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
pub fn object_key(workspace: &str, operation: &str, name: &str) -> String {
    format!(
        "workspaces/{}/archives/{}/{}",
        crate::objects::encode(workspace, false),
        crate::objects::encode(operation, false),
        name
    )
}
pub(super) fn archive(row: &Value) -> Result<RecoveryArchive, ApiError> {
    let a: RecoveryArchive =
        serde_json::from_value(row["request_config"]["recoveryArchive"].clone())
            .map_err(|_| fenced())?;
    if !a.valid_for(
        required(row, "workspace_id")?,
        required(row, "chat_id")?,
        required(row, "initial_session_id")?,
    ) {
        return Err(fenced());
    }
    Ok(a)
}
fn capability(master: &MasterKey, row: &Value, a: &RecoveryArchive) -> String {
    master.digest_api(
        "workspace-archive",
        &json!([
            row["account_id"],
            row["workspace_id"],
            a.id,
            a.expires_at_ms,
            if a.phase == ArchivePhase::Restoring {
                "read"
            } else {
                "write"
            }
        ])
        .to_string(),
    )
}
pub(super) async fn locked_op(
    tx: &mut Transaction<'_, Postgres>,
    row: &Value,
) -> Result<Value, ApiError> {
    let op:Option<Value>=sqlx::query_scalar("SELECT to_jsonb(o) FROM zuse_rust_recovery_operations o WHERE operation_id=$1 AND workspace_id=$2 FOR UPDATE")
        .bind(row["request_config"]["recoveryArchive"]["id"].as_str()).bind(required(row,"workspace_id")?).fetch_optional(&mut **tx).await?;
    let op = op.ok_or_else(fenced)?;
    if deleted(row)
        || op["source_machine_id"] != row["request_config"]["recoveryArchive"]["sourceSandboxId"]
        || op["binding"]["workspaceId"] != row["workspace_id"]
        || op["binding"]["chatId"] != row["chat_id"]
        || op["binding"]["sessionId"] != row["initial_session_id"]
        || op["binding"]["rustStorage"]["accountId"] != row["account_id"]
        || op
            .get("restore_generation")
            .filter(|v| !v.is_null())
            .unwrap_or(&op["generation"])
            != &row["request_config"]["runtimeGeneration"]
        || op["storage_incarnation_id"]
            != row["request_config"]["rustRuntimeStorage"]["storageIncarnationId"]
        || !matches!(
            op["phase"].as_str(),
            Some("retired" | "restoring" | "restored")
        ) && op["source_machine_id"] != row["provider_sandbox_id"]
    {
        return Err(fenced());
    }
    Ok(op)
}
pub(super) async fn save(
    tx: &mut Transaction<'_, Postgres>,
    row: &Value,
    a: &RecoveryArchive,
    status: &str,
    now: i64,
) -> Result<(), ApiError> {
    let mut config = row["request_config"].clone();
    config["recoveryArchive"] = serde_json::to_value(a).map_err(|_| fenced())?;
    sqlx::query("UPDATE api_cloud_workspaces SET request_config=$2,status_code=$3,revision=revision+1,updated_at=GREATEST(updated_at+1,$4) WHERE workspace_id=$1").bind(required(row,"workspace_id")?).bind(config).bind(status).bind(now).execute(&mut **tx).await?;
    Ok(())
}
/// Private worker input; never returned by user/workspace read APIs or logged.
pub struct CapturePlan {
    pub operation_id: String,
    pub workspace_id: String,
    pub source_machine_id: String,
    pub config: Zeroizing<String>,
}
#[async_trait]
pub trait RecoveryProvider: Send + Sync {
    fn provider_id(&self) -> &'static str {
        "boxd"
    }
    async fn inspect(&self, id: &str) -> Result<Option<Machine>, ProviderError>;
    async fn wake(&self, id: &str, state: &MachineState) -> Result<(), ProviderError>;
    async fn capture(&self, plan: &CapturePlan) -> Result<(), ProviderError>;
    async fn stop_capture(&self, id: &str, operation: &str) -> Result<(), ProviderError>;
    async fn pause(&self, id: &str) -> Result<(), ProviderError>;
    async fn retire(&self, id: &str) -> Result<(), ProviderError>;
    async fn pause_receipt(&self, id: &str) -> Result<ProviderOperation, ProviderError> {
        self.pause(id).await?;
        Ok(ProviderOperation {
            resource_id: id.into(),
            operation_id: None,
            desired: DesiredState::Sleeping,
        })
    }
    async fn wake_receipt(
        &self,
        id: &str,
        state: &MachineState,
    ) -> Result<ProviderOperation, ProviderError> {
        self.wake(id, state).await?;
        Ok(ProviderOperation {
            resource_id: id.into(),
            operation_id: None,
            desired: DesiredState::Running,
        })
    }
    async fn recovery_state(
        &self,
        receipt: &ProviderOperation,
    ) -> Result<OperationState, ProviderError> {
        if receipt.operation_id.is_some() {
            return Err(ProviderError::InvalidResponse);
        }
        let machine = self.inspect(&receipt.resource_id).await?;
        let completed = match (receipt.desired, machine.as_ref().map(|m| &m.state)) {
            (DesiredState::Deleted, None) => true,
            (DesiredState::Running, Some(MachineState::Running)) => true,
            (
                DesiredState::Sleeping,
                Some(MachineState::Standby | MachineState::Paused | MachineState::Stopped),
            ) => true,
            _ => false,
        };
        Ok(if completed {
            OperationState::Completed
        } else {
            OperationState::Pending
        })
    }
    /// Where the capture helper launched for `operation` stands on the guest.
    /// Adapters without a guest status read report it running; the upload
    /// time limit still bounds it.
    async fn capture_status(
        &self,
        _id: &str,
        _operation: &str,
    ) -> Result<CaptureStatus, ProviderError> {
        Ok(CaptureStatus::Running)
    }
    async fn retire_receipt(&self, id: &str) -> Result<ProviderOperation, ProviderError> {
        self.retire(id).await?;
        Ok(ProviderOperation {
            resource_id: id.into(),
            operation_id: None,
            desired: DesiredState::Deleted,
        })
    }
    async fn retirement_state(
        &self,
        receipt: &ProviderOperation,
    ) -> Result<OperationState, ProviderError> {
        if receipt.operation_id.is_some() || receipt.desired != DesiredState::Deleted {
            return Err(ProviderError::InvalidResponse);
        }
        Ok(if self.inspect(&receipt.resource_id).await?.is_none() {
            OperationState::Completed
        } else {
            OperationState::Pending
        })
    }
}
impl RuntimeControl {
    pub fn with_recovery_policy(mut self, enabled: bool, workspaces: Vec<String>) -> Self {
        self.recovery_enabled = enabled;
        self.recovery_workspaces = workspaces;
        self
    }
    pub async fn recovery_ready(&self) -> Result<(), ApiError> {
        let ready: bool =
            sqlx::query_scalar("SELECT to_regclass('zuse_rust_recovery_operations') IS NOT NULL AND to_regclass('zuse_rust_restores') IS NOT NULL AND to_regclass('api_cloud_usage_outbox') IS NOT NULL AND to_regclass('api_cloud_runtime_observations') IS NOT NULL AND EXISTS(SELECT 1 FROM information_schema.columns WHERE table_schema=current_schema() AND table_name='zuse_rust_recovery_operations' AND column_name='provider_effects') AND EXISTS(SELECT 1 FROM information_schema.columns WHERE table_schema=current_schema() AND table_name='zuse_rust_restores' AND column_name='provider_effects')")
                .fetch_one(&self.pool)
                .await?;
        if !ready {
            return Err(ApiError::unavailable("cloud_recovery_schema_not_ready"));
        }
        Ok(())
    }
    fn recovery_allowed(&self, id: &str) -> bool {
        self.recovery_enabled
            && (self.recovery_workspaces.is_empty()
                || self.recovery_workspaces.iter().any(|s| s == id))
    }
    pub(super) fn capture_plan(
        &self,
        row: &Value,
        a: &RecoveryArchive,
        binding: Value,
    ) -> Result<CapturePlan, ApiError> {
        let key = self
            .master
            .open_transcript_key(
                required(row, "account_id")?,
                required(row, "workspace_id")?,
                required(row, "wrapped_transcript_key")?,
            )
            .map_err(|_| failed())?;
        let mut config = json!({"operationId":a.id,"workspaceId":row["workspace_id"],"chatId":row["chat_id"],"sessionId":row["initial_session_id"],"mode":"backup","expiresAtMs":a.expires_at_ms,"home":"/home/zuse","repositoriesPath":"/home/zuse/workspace","url":format!("{}/v1/cloud/workspaces/{}/archive/{}",self.signer.issuer(),crate::objects::encode(required(row,"workspace_id")?,false),a.id),"token":capability(&self.master,row,a),"key":&*key});
        config["rustStorage"] = binding["rustStorage"].clone();
        config["supervisorBinding"] = json!({"workspaceId":row["workspace_id"],"generation":row["request_config"]["runtimeGeneration"],"storageIncarnationId":row["request_config"]["rustRuntimeStorage"]["storageIncarnationId"]});
        Ok(CapturePlan {
            operation_id: a.id.clone(),
            workspace_id: required(row, "workspace_id")?.into(),
            source_machine_id: a.source_sandbox_id.clone(),
            config: Zeroizing::new(config.to_string()),
        })
    }
    /// Expiry only admits a recovery job. It never independently authorizes destruction.
    pub async fn begin_recovery(
        &self,
        id: &str,
        now: i64,
    ) -> Result<Option<CapturePlan>, ApiError> {
        if !self.recovery_allowed(id) {
            return Ok(None);
        }
        self.recovery_ready().await?;
        if self.transcript_objects.is_none() {
            return Err(ApiError::unavailable("cloud_archive_store_unavailable"));
        }
        let mut tx = self.pool.begin().await?;
        let (row, generation, _) = locked_workspace(&mut tx, id).await?;
        let config = &row["request_config"];
        if let Ok(mut a) = archive(&row) {
            if a.phase != ArchivePhase::Restored {
                let op = locked_op(&mut tx, &row).await?;
                if matches!(op["phase"].as_str(), Some("uploading" | "verifying")) {
                    if a.expires_at_ms <= now {
                        a.expires_at_ms = now + TTL;
                        sqlx::query("UPDATE zuse_rust_recovery_operations SET expires_at=$2,updated_at=$3 WHERE operation_id=$1").bind(&a.id).bind(a.expires_at_ms).bind(now).execute(&mut *tx).await?;
                        save(&mut tx, &row, &a, "archive-uploading", now).await?;
                    }
                    let plan = self.capture_plan(&row, &a, op["binding"].clone())?;
                    tx.commit().await?;
                    return Ok(Some(plan));
                }
                return Ok(None);
            }
        }
        let due = crate::archive::status(&row)
            .retention_deadline
            .is_some_and(|at| at <= now);
        let caps = &config["rustStorageEnrollment"]["capabilities"];
        if deleted(&row)
            || (!super::recreation::capture_intent(&row)
                && (row["state"] != "archived" || row["desired_state"] != "archived" || !due))
            || !(row["provider"] == "boxd"
                || config["rustProviderControllerVersion"] == 1
                    && matches!(row["provider"].as_str(), Some("boat" | "e2b" | "daytona")))
            || !storage_enrolled(&row)
            || config["rustLifecycleOperation"]["phase"] != "paused"
            || config["providerAuthMode"] != "broker-v1"
            || !caps
                .as_array()
                .is_some_and(|a| a.iter().any(|v| v == CAPABILITY))
            || config["rustRuntimeStorage"]["format"]
                != zuse_protocol::cloud::RUST_RUNTIME_STORAGE_FORMAT
        {
            return Ok(None);
        }
        let shared:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM api_cloud_workspaces WHERE workspace_id<>$1 AND provider=$2 AND provider_sandbox_id=$3 AND state<>'deleted')").bind(id).bind(required(&row,"provider")?).bind(required(&row,"provider_sandbox_id")?).fetch_one(&mut *tx).await?;
        if shared {
            return Ok(None);
        }
        let operation = uuid::Uuid::new_v4().to_string();
        let account = required(&row, "account_id")?;
        let sanitize = |s: &str| {
            s.chars()
                .map(|c| {
                    if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                        c
                    } else {
                        '_'
                    }
                })
                .collect::<String>()
        };
        let binding = Binding {
            operation_id: operation.clone(),
            workspace_id: id.into(),
            chat_id: required(&row, "chat_id")?.into(),
            session_id: required(&row, "initial_session_id")?.into(),
            rust_storage: Some(RustStorage {
                database: format!("orgs/{}/{}/docs.sqlite3", sanitize(id), sanitize(account))
                    .into(),
                account_id: account.into(),
                storage_incarnation_id: required(
                    &config["rustRuntimeStorage"],
                    "storageIncarnationId",
                )?
                .into(),
            }),
        };
        let a = RecoveryArchive {
            version: 1,
            id: operation.clone(),
            source_sandbox_id: required(&row, "provider_sandbox_id")?.into(),
            archived_at_ms: row["archive_requested_at"].as_i64().unwrap_or(now),
            expires_at_ms: now + TTL,
            phase: ArchivePhase::Uploading,
            verified_chunks: 0,
            manifest: None,
        };
        let binding = serde_json::to_value(binding).map_err(|_| failed())?;
        sqlx::query("INSERT INTO zuse_rust_recovery_operations(operation_id,workspace_id,generation,source_machine_id,storage_incarnation_id,binding,source_credential_hash,phase,expires_at,created_at,updated_at) VALUES($1,$2,$3,$4,$5,$6,$7,'uploading',$8,$9,$9)").bind(&operation).bind(id).bind(generation).bind(&a.source_sandbox_id).bind(config["rustRuntimeStorage"]["storageIncarnationId"].as_str()).bind(&binding).bind(row["runtime_credential_hash"].as_str()).bind(a.expires_at_ms).bind(now).execute(&mut *tx).await?;
        save(&mut tx, &row, &a, "archive-uploading", now).await?;
        // The archive path was already quiesced; revoke any old runtime bearer before waking for capture.
        sqlx::query(
            "UPDATE api_cloud_workspaces SET runtime_credential_hash=NULL WHERE workspace_id=$1",
        )
        .bind(id)
        .execute(&mut *tx)
        .await?;
        sqlx::query("DELETE FROM zuse_rust_gateway_leases WHERE workspace_id=$1")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        jobs::enqueue(
            &mut tx,
            &format!("recovery:{operation}"),
            JOB_KIND,
            id,
            generation,
            &json!({"operationId":operation}),
            now,
        )
        .await?;
        let plan = self.capture_plan(&row, &a, binding)?;
        tx.commit().await?;
        Ok(Some(plan))
    }
    pub async fn archive_object(
        &self,
        id: &str,
        operation: &str,
        name: &str,
        token: &str,
        body: Option<Vec<u8>>,
        now: i64,
    ) -> Result<Option<Vec<u8>>, ApiError> {
        if !valid_object(name) || token.len() > 512 {
            return Err(ApiError::unauthorized(
                "workspace_archive_capability_invalid",
            ));
        }
        let objects = self
            .transcript_objects
            .as_ref()
            .ok_or_else(|| ApiError::unavailable("cloud_archive_store_unavailable"))?;
        let mut tx = self.pool.begin().await?;
        let row = if body.is_some() {
            locked_workspace(&mut tx, id).await?.0
        } else {
            sqlx::query_scalar::<_, Value>(
                "SELECT to_jsonb(w) FROM api_cloud_workspaces w WHERE workspace_id=$1 FOR UPDATE",
            )
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or_else(|| ApiError::unauthorized("workspace_archive_capability_invalid"))?
        };
        let mut a = archive(&row)
            .map_err(|_| ApiError::unauthorized("workspace_archive_capability_invalid"))?;
        if deleted(&row)
            || a.id != operation
            || a.expires_at_ms <= now
            || !equal(token, &capability(&self.master, &row, &a))
        {
            return Err(ApiError::unauthorized(
                "workspace_archive_capability_invalid",
            ));
        }
        let key = object_key(id, operation, name);
        if let Some(body) = body {
            if body.is_empty()
                || body.len()
                    > if name == "manifest" {
                        1024 * 1024
                    } else {
                        WIRE_LIMIT
                    }
            {
                return Err(ApiError::bad_request("workspace_archive_object_invalid"));
            }
            let op = locked_op(&mut tx, &row).await?;
            if (row["desired_state"] != "archived" && !super::recreation::capture_intent(&row))
                || !matches!(op["phase"].as_str(), Some("uploading" | "verifying"))
                || !matches!(a.phase, ArchivePhase::Uploading | ArchivePhase::Verifying)
            {
                return Err(fenced());
            }
            if name == "manifest" {
                let manifest: ArchiveManifest = serde_json::from_slice(&body)
                    .map_err(|_| ApiError::bad_request("workspace_archive_manifest_invalid"))?;
                if !manifest.valid()
                    || manifest.operation_id != operation
                    || manifest.workspace_id != id
                    || manifest.chat_id != row["chat_id"]
                    || manifest.session_id != row["initial_session_id"]
                {
                    return Err(ApiError::bad_request("workspace_archive_identity_mismatch"));
                }
                if let Some(prior) = &a.manifest {
                    if serde_json::to_value(prior).map_err(|_| failed())?
                        != serde_json::to_value(&manifest).map_err(|_| failed())?
                    {
                        return Err(ApiError::conflict("workspace_archive_manifest_conflict"));
                    }
                }
                objects.put_archive(&key, &body).await?;
                a.manifest = Some(manifest);
                a.phase = ArchivePhase::Verifying;
                sqlx::query("UPDATE zuse_rust_recovery_operations SET phase='verifying',manifest=$2,updated_at=$3 WHERE operation_id=$1").bind(operation).bind(serde_json::to_value(&a.manifest).map_err(|_|failed())?).bind(now).execute(&mut *tx).await?;
                save(&mut tx, &row, &a, "archive-verifying", now).await?;
            } else {
                if hash_bytes(&body) != name {
                    return Err(ApiError::bad_request("workspace_archive_checksum_mismatch"));
                }
                objects.put_archive(&key, &body).await?;
            }
            tx.commit().await?;
            Ok(None)
        } else {
            if a.phase != ArchivePhase::Restoring {
                return Err(ApiError::unauthorized(
                    "workspace_archive_operation_inactive",
                ));
            }
            let bytes = objects
                .get_archive(&key)
                .await?
                .ok_or_else(|| ApiError::not_found("workspace_archive_object_missing"))?;
            if bytes.len()
                > if name == "manifest" {
                    1024 * 1024
                } else {
                    WIRE_LIMIT
                }
            {
                return Err(failed());
            }
            tx.commit().await?;
            Ok(Some(bytes))
        }
    }
}
fn hash_bytes(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}
/// The capture helper's state on the guest, read from its status directory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaptureStatus {
    /// Its process is alive.
    Running,
    /// It published the archive (`uploaded`).
    Uploaded,
    /// It gave up (`failed`).
    Failed,
    /// Not running and no result: not started yet, or ended without one.
    Idle,
}
/// Reads the helper's status directory `$1` as the guest task user.
const CAPTURE_STATUS_SCRIPT: &str = r#"d="$1"; if [ -f "$d/uploaded" ]; then echo uploaded; elif [ -f "$d/failed" ]; then echo failed; elif [ -f "$d/pid" ] && read p <"$d/pid" && kill -0 "$p" 2>/dev/null; then echo running; else echo idle; fi"#;
pub(super) fn capture_status_command(run_as: &str, status_root: &str, operation: &str) -> String {
    format!(
        "{run_as} sh -c {} sh {}",
        quote(CAPTURE_STATUS_SCRIPT),
        quote(&format!("{status_root}/{operation}"))
    )
}
pub(super) fn capture_status(
    result: &crate::providers::ExecResult,
) -> Result<CaptureStatus, ProviderError> {
    if result.exit_code != 0 {
        return Err(ProviderError::Unavailable);
    }
    match std::str::from_utf8(&result.stdout).map(str::trim) {
        Ok("running") => Ok(CaptureStatus::Running),
        Ok("uploaded") => Ok(CaptureStatus::Uploaded),
        Ok("failed") => Ok(CaptureStatus::Failed),
        Ok("idle") => Ok(CaptureStatus::Idle),
        _ => Err(ProviderError::InvalidResponse),
    }
}
/// Launches one capture attempt. A previous attempt's `failed` marker is
/// cleared so the status read reflects this attempt only.
pub(super) fn capture_command(plan: &CapturePlan) -> (String, String) {
    let staged = format!("/tmp/zuse-recovery-{}", plan.operation_id);
    let directory = format!("/var/lib/zuse/recovery/{}", plan.operation_id);
    let command = format!(
        "set -eu\nsudo -n install -d -o zuse -g zuse -m 700 {}\nsudo -n install -o zuse -g zuse -m 600 {} {}/config.json\nsudo -n rm -- {}\nsudo -n rm -f -- {}/failed\nsudo -n -u zuse /usr/local/lib/zuse/launch-rust-recovery {}",
        quote(&directory),
        quote(&staged),
        quote(&directory),
        quote(&staged),
        quote(&directory),
        quote(&plan.operation_id)
    );
    (staged, command)
}
#[async_trait]
impl RecoveryProvider for crate::providers::boxd::Boxd {
    async fn inspect(&self, id: &str) -> Result<Option<Machine>, ProviderError> {
        self.inspect(id).await
    }
    async fn wake(&self, id: &str, state: &MachineState) -> Result<(), ProviderError> {
        self.activate(id, state).await
    }
    async fn capture(&self, plan: &CapturePlan) -> Result<(), ProviderError> {
        let (staged, command) = capture_command(plan);
        self.upload(&plan.source_machine_id, &staged, plan.config.as_bytes())
            .await?;
        if self
            .exec(&plan.source_machine_id, &command)
            .await?
            .exit_code
            != 0
        {
            return Err(ProviderError::Unavailable);
        }
        Ok(())
    }
    async fn stop_capture(&self, id: &str, operation: &str) -> Result<(), ProviderError> {
        if self
            .exec(
                id,
                &format!(
                    "sudo -n -u zuse /usr/local/lib/zuse/launch-rust-recovery {} --cancel",
                    quote(operation)
                ),
            )
            .await?
            .exit_code
            != 0
        {
            return Err(ProviderError::Unavailable);
        }
        Ok(())
    }
    async fn capture_status(
        &self,
        id: &str,
        operation: &str,
    ) -> Result<CaptureStatus, ProviderError> {
        let command =
            capture_status_command("sudo -n -u zuse env", "/var/lib/zuse/recovery", operation);
        capture_status(&self.exec(id, &command).await?)
    }
    async fn pause(&self, id: &str) -> Result<(), ProviderError> {
        self.hibernate(id).await
    }
    async fn retire(&self, id: &str) -> Result<(), ProviderError> {
        self.destroy(id).await
    }
}
struct Reader {
    objects: Arc<dyn Objects>,
    workspace: String,
    operation: String,
    pool: PgPool,
    job: jobs::Job,
}
#[async_trait]
impl zuse_cloud_runtime::recovery::Objects for Reader {
    async fn get(&self, name: &str) -> anyhow::Result<Vec<u8>> {
        anyhow::ensure!(valid_object(name), "archive_object_invalid");
        // Cancellation/deletion stops additional privileged chunk reads, not only the final barrier write.
        let allowed:bool=sqlx::query_scalar(&format!("SELECT EXISTS(SELECT 1 FROM api_cloud_workspaces w JOIN zuse_rust_control_owners o ON o.resource_kind='workspace' AND o.resource_id=w.workspace_id AND o.backend='rust' AND o.generation=$2 WHERE w.workspace_id=$1 AND {} AND w.state NOT IN ('deleting','deleted') AND w.request_config->'recoveryArchive'->>'id'=$3 AND EXISTS(SELECT 1 FROM zuse_rust_jobs j WHERE j.job_id=$4 AND j.lease_token=$5 AND j.state='running' AND j.lease_expires_at>$6))",super::recreation::CAPTURE_SQL)).bind(&self.workspace).bind(self.job.generation).bind(&self.operation).bind(&self.job.id).bind(&self.job.lease_token).bind(clock_ms()).fetch_one(&self.pool).await?;
        anyhow::ensure!(allowed, "archive_verification_fenced");
        self.objects
            .get_archive(&object_key(&self.workspace, &self.operation, name))
            .await
            .map_err(|_| anyhow::anyhow!("archive_object_unavailable"))?
            .ok_or_else(|| anyhow::anyhow!("archive_object_missing"))
    }
    async fn put(&self, _: &str, _: &[u8]) -> anyhow::Result<()> {
        anyhow::bail!("archive_verifier_read_only")
    }
}
pub(super) async fn active_job(
    tx: &mut Transaction<'_, Postgres>,
    job: &jobs::Job,
) -> Result<(), ApiError> {
    let active:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM zuse_rust_jobs WHERE job_id=$1 AND lease_token=$2 AND state='running' AND lease_expires_at>$3 FOR UPDATE)").bind(&job.id).bind(&job.lease_token).bind(clock_ms()).fetch_one(&mut **tx).await?;
    if !active {
        return Err(fenced());
    }
    Ok(())
}
pub(super) async fn defer(
    tx: &mut Transaction<'_, Postgres>,
    job: &jobs::Job,
    complete: bool,
    now: i64,
) -> Result<(), ApiError> {
    let saved=sqlx::query("UPDATE zuse_rust_jobs SET state=$3,lease_token=NULL,lease_expires_at=NULL,due_at=$4,updated_at=$5,attempts=CASE WHEN $3='pending' THEN 0 ELSE attempts END WHERE job_id=$1 AND lease_token=$2 AND state='running' AND lease_expires_at>$6").bind(&job.id).bind(&job.lease_token).bind(if complete{"completed"}else{"pending"}).bind(now+2000).bind(now).bind(clock_ms()).execute(&mut **tx).await?;
    if saved.rows_affected() != 1 {
        return Err(fenced());
    }
    Ok(())
}
impl RuntimeControl {
    pub async fn recovery_once(
        &self,
        provider: &dyn RecoveryProvider,
        now: i64,
    ) -> Result<bool, ApiError> {
        let ready: bool =
            sqlx::query_scalar("SELECT to_regclass('zuse_rust_recovery_operations') IS NOT NULL")
                .fetch_one(&self.pool)
                .await?;
        if !ready {
            return Ok(false);
        }
        self.recovery_admit_due(now).await?;
        let Some(job) = jobs::claim(&self.pool, JOB_KIND, "workspace", now, LEASE).await? else {
            return Ok(false);
        };
        self.recovery_run_job(provider, &job, now).await
    }
    pub(super) async fn recovery_admit_due(&self, now: i64) -> Result<(), ApiError> {
        if self.recovery_enabled {
            let ids:Vec<String>=sqlx::query_scalar("SELECT w.workspace_id FROM api_cloud_workspaces w JOIN zuse_rust_control_owners o ON o.resource_kind='workspace' AND o.resource_id=w.workspace_id AND o.backend='rust' WHERE w.state='archived' AND w.desired_state='archived' AND w.provider_sandbox_id IS NOT NULL AND (w.archive_requested_at+864000000 <= $1 OR w.archive_requested_at IS NULL AND w.archive_delete_at<=$1) AND NOT EXISTS(SELECT 1 FROM zuse_rust_recovery_operations r WHERE r.workspace_id=w.workspace_id AND r.phase NOT IN ('cancelled','restored')) ORDER BY w.archive_delete_at LIMIT 10").bind(now).fetch_all(&self.pool).await?;
            for id in ids {
                self.begin_recovery(&id, now).await?;
            }
        }
        Ok(())
    }
    pub(super) async fn recovery_run_job(
        &self,
        provider: &dyn RecoveryProvider,
        job: &jobs::Job,
        now: i64,
    ) -> Result<bool, ApiError> {
        let row: Value = sqlx::query_scalar(
            "SELECT to_jsonb(w) FROM api_cloud_workspaces w WHERE workspace_id=$1",
        )
        .bind(&job.resource_id)
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| ApiError::not_found("cloud_workspace_not_found"))?;
        self.recreation_effect_scope(&row).await?;
        let result = self.recovery_step(provider, job, now).await;
        if let Err(error) = &result {
            // A failed delivery may have lost its lease or generation. Never
            // issue provider cleanup from this error path: the next admitted
            // state-machine step reconciles the helper and original operation.
            let mut tx = self.pool.begin().await?;
            if let Ok((row, generation, _)) = locked_workspace(&mut tx, &job.resource_id).await {
                if generation == job.generation
                    && active_job(&mut tx, &job).await.is_ok()
                    && !deleted(&row)
                    && (row["desired_state"] == "archived"
                        || super::recreation::capture_intent(&row))
                    && row["request_config"]["recoveryArchive"]["id"] == job.payload["operationId"]
                {
                    sqlx::query("UPDATE api_cloud_workspaces SET status_code=$2,revision=revision+1,updated_at=GREATEST(updated_at+1,$3) WHERE workspace_id=$1")
                        .bind(&job.resource_id).bind(error.code).bind(clock_ms()).execute(&mut *tx).await?;
                }
            }
            tx.commit().await?;
            jobs::retry(&self.pool, &job, clock_ms(), error.code).await?;
        }
        result.map(|_| true)
    }
    async fn recovery_step(
        &self,
        provider: &dyn RecoveryProvider,
        job: &jobs::Job,
        now: i64,
    ) -> Result<(), ApiError> {
        let effects = effects::Guard::acquire(&job.resource_id)?;
        let mut tx = self.pool.begin().await?;
        sqlx::query("SELECT generation FROM zuse_rust_control_owners WHERE resource_kind='workspace' AND resource_id=$1 AND backend='rust' FOR UPDATE").bind(&job.resource_id).fetch_optional(&mut *tx).await?;
        let (row, generation, _) = locked_workspace(&mut tx, &job.resource_id).await?;
        active_job(&mut tx, job).await?;
        if deleted(&row) || generation != job.generation {
            return Err(fenced());
        }
        let op_id = job.payload["operationId"].as_str().ok_or_else(fenced)?;
        let op:Value=sqlx::query_scalar("SELECT to_jsonb(o) FROM zuse_rust_recovery_operations o WHERE operation_id=$1 AND workspace_id=$2 FOR UPDATE").bind(op_id).bind(&job.resource_id).fetch_one(&mut *tx).await?;
        if op["generation"] != generation
            || op["binding"]["rustStorage"]["accountId"] != row["account_id"]
            || op["binding"]["chatId"] != row["chat_id"]
            || op["binding"]["sessionId"] != row["initial_session_id"]
        {
            return Err(fenced());
        }
        if row["provider"] != provider.provider_id() {
            return Err(ApiError::conflict("cloud_provider_identity_mismatch"));
        }
        if op["phase"] == "cancelled" {
            tx.commit().await?;
            return self.recovery_cancel(provider, job).await;
        }
        let a = archive(&row)?;
        let current = locked_op(&mut tx, &row).await?;
        if a.id != op_id || current["operation_id"] != op_id {
            return Err(fenced());
        }
        match op["phase"].as_str() {
            Some("uploading") => {
                tx.commit().await?;
                return self.recovery_upload(provider, job).await;
            }
            Some("verifying") => {
                tx.commit().await?;
                if !self.recovery_prepare_verification(provider, job).await? {
                    return Ok(());
                }
                let (tx, row, op) = self.recovery_stage_locked(job, "verifying").await?;
                let key = self
                    .master
                    .open_transcript_key(
                        required(&row, "account_id")?,
                        &job.resource_id,
                        required(&row, "wrapped_transcript_key")?,
                    )
                    .map_err(|_| failed())?;
                let binding: Binding =
                    serde_json::from_value(op["binding"].clone()).map_err(|_| fenced())?;
                let reader = Reader {
                    objects: self.transcript_objects.as_ref().ok_or_else(failed)?.clone(),
                    workspace: job.resource_id.clone(),
                    operation: op_id.into(),
                    pool: self.pool.clone(),
                    job: job.clone(),
                };
                tx.commit().await?;
                let handle = tokio::runtime::Handle::current();
                let pool = self.pool.clone();
                let resource = job.resource_id.clone();
                let expected_generation = job.generation;
                let task = tokio::task::spawn_blocking(move || {
                    handle.block_on(async move {
                        // Keep the execution guard through TempDir destruction, even if the job lease is revoked.
                        let guard=effects;
                        let allowed:bool=sqlx::query_scalar(&format!("SELECT EXISTS(SELECT 1 FROM api_cloud_workspaces w JOIN zuse_rust_control_owners o ON o.resource_id=w.workspace_id AND o.resource_kind='workspace' AND o.backend='rust' AND o.generation=$2 WHERE w.workspace_id=$1 AND {} AND w.state NOT IN ('deleting','deleted'))",super::recreation::CAPTURE_SQL)).bind(&resource).bind(expected_generation).fetch_one(&pool).await?;
                        anyhow::ensure!(allowed,"archive_verification_fenced");
                        let temp = guard.temp()?;
                        let root = temp.path();
                        let home = root.join("home");
                        let data = root.join("data");
                        let repositories = root.join("repositories");
                        for p in [&home, &data, &repositories] {
                            std::fs::create_dir(p)?;
                        }
                        let helper = zuse_cloud_runtime::recovery::Recovery::acquire(
                            binding,
                            zuse_cloud_runtime::recovery::Layout {
                                home,
                                data,
                                repositories,
                                work: root.join("verification"),
                            },
                            zuse_cloud_runtime::recovery::Key::parse(&key)?,
                        )?;
                        helper.verify(&reader).await
                    })
                });
                tokio::pin!(task);
                let verified = loop {
                    tokio::select! {result=&mut task=>break result.map_err(|_|ApiError::unavailable("archive-verification-failed"))?.map_err(|_|ApiError::unavailable("archive-verification-failed"))?,_=tokio::time::sleep(Duration::from_secs(30))=>{if !jobs::extend(&self.pool,job,"workspace",clock_ms(),LEASE).await?{return Err(fenced())}}}
                };
                let mut tx = self.pool.begin().await?;
                let (row, generation, _) = locked_workspace(&mut tx, &job.resource_id).await?;
                active_job(&mut tx, job).await?;
                let op = locked_op(&mut tx, &row).await?;
                let mut a = archive(&row)?;
                if generation != job.generation
                    || a.id != op_id
                    || (row["desired_state"] != "archived"
                        && !super::recreation::capture_intent(&row))
                    || op["phase"] != "verifying"
                    || serde_json::to_value(&verified).map_err(|_| failed())? != op["manifest"]
                {
                    return Err(fenced());
                }
                a.phase = ArchivePhase::Retiring;
                a.verified_chunks = verified.chunks.len();
                a.manifest = Some(verified.clone());
                let now = clock_ms();
                sqlx::query("UPDATE zuse_rust_recovery_operations SET phase='retiring',verified_at=$2,verified_archive_sha256=$3,updated_at=$2 WHERE operation_id=$1").bind(op_id).bind(now).bind(&verified.archive_sha256).execute(&mut *tx).await?;
                save(&mut tx, &row, &a, "archive-retiring", now).await?;
                defer(&mut tx, job, false, now - 2000).await?;
                tx.commit().await?;
                return Ok(());
            }
            Some("retiring") => {
                tx.commit().await?;
                return self.recovery_retirement(provider, job, now).await;
            }
            Some("retired")
                if row["request_config"]["rustRestoreRequested"].is_string()
                    && row["desired_state"] == "ready" =>
            {
                self.queue_restore_tx(
                    &mut tx,
                    &row,
                    row["request_config"]["rustRestoreRequested"]
                        .as_str()
                        .unwrap(),
                    now,
                )
                .await?;
                defer(&mut tx, job, true, now).await?;
                tx.commit().await?;
                return Ok(());
            }
            Some("retired" | "restored") => {
                defer(&mut tx, job, true, now).await?;
                tx.commit().await?;
                return Ok(());
            }
            _ => return Err(fenced()),
        }
    }
}

impl RuntimeControl {
    /// Runs inside the ordinary authorized lifecycle transaction, before retained-machine checks.
    pub(super) async fn recovery_action(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        row: &Value,
        action: zuse_protocol::cloud_lifecycle::Action,
        command: &str,
        now: i64,
    ) -> Result<bool, ApiError> {
        use zuse_protocol::cloud_lifecycle::Action;
        if row["request_config"]["rustRecoveryCancellation"].is_string()
            && action != Action::Unarchive
        {
            return Err(ApiError::conflict("archive-cancellation-pending"));
        }
        let Ok(mut a) = archive(row) else {
            return Ok(false);
        };
        if a.phase == ArchivePhase::Restored {
            return Ok(false);
        }
        let op = locked_op(tx, row).await?;
        let prior:Option<String>=sqlx::query_scalar("SELECT action FROM zuse_rust_lifecycle_operations WHERE workspace_id=$1 AND command_id=$2").bind(required(row,"workspace_id")?).bind(command).fetch_optional(&mut **tx).await?;
        if let Some(prior) = prior {
            if prior != action.as_str() {
                return Err(ApiError::conflict("cloud_lifecycle_command_id_reused"));
            }
            return Ok(true);
        }
        // Repeated unarchive remains a read-like acknowledgement once interactive restore is accepted.
        if action == Action::Unarchive
            && (op["phase"] == "restoring"
                || row["request_config"]["rustRestoreRequested"].is_string())
        {
            return Ok(true);
        }
        if action == Action::Resume
            && matches!(
                op["phase"].as_str(),
                Some("retiring" | "retired" | "restoring")
            )
        {
            if row["desired_state"] == "archived" {
                return Err(ApiError::conflict("cloud_workspace_unarchive_required"));
            }
            crate::images::deletion::snapshot_read_guard(tx, required(row, "account_id")?, None)
                .await?;
            lifecycle::require_resume_capacity(self.staging_candidate.as_deref(), tx, row, now)
                .await?;
            if op["phase"] == "restoring" {
                sqlx::query("UPDATE zuse_rust_jobs SET state='pending',attempts=0,due_at=$3,last_error_code=NULL,updated_at=$3 WHERE kind=$1 AND resource_id=$2 AND resource_generation=$4 AND payload->>'operationId'=$5 AND state='failed'")
                    .bind(restore::JOB_KIND).bind(required(row,"workspace_id")?).bind(now).bind(row["request_config"]["runtimeGeneration"].as_i64()).bind(row["request_config"]["rustRestoreOperationId"].as_str()).execute(&mut **tx).await?;
                return Ok(true);
            }
            if op["phase"] == "retiring" {
                sqlx::query("UPDATE api_cloud_workspaces SET desired_state='ready',state='provisioning',request_config=jsonb_set(request_config,'{rustRestoreRequested}',$2),revision=revision+1 WHERE workspace_id=$1").bind(required(row,"workspace_id")?).bind(json!(command)).execute(&mut **tx).await?;
            } else {
                self.queue_restore_tx(tx, row, command, now).await?;
            }
            return Ok(true);
        }
        if action != Action::Unarchive && !(action == Action::Archive && op["phase"] == "retired") {
            return Err(ApiError::conflict("archive-operation-in-progress"));
        }
        let operation = uuid::Uuid::new_v4().to_string();
        let generation = op["generation"].as_i64().ok_or_else(fenced)?;
        let mut config = row["request_config"].clone();
        if matches!(op["phase"].as_str(), Some("uploading" | "verifying")) {
            sqlx::query("UPDATE zuse_rust_recovery_operations SET phase='cancelled',updated_at=$2 WHERE operation_id=$1").bind(&a.id).bind(now).execute(&mut **tx).await?;
            config
                .as_object_mut()
                .ok_or_else(fenced)?
                .remove("recoveryArchive");
            config["rustRecoveryCancellation"] = json!(a.id);
            // Revoke the verifier's delivery token. Cleanup is a fresh, durable delivery of the same operation.
            sqlx::query("UPDATE zuse_rust_jobs SET state='pending',lease_token=NULL,lease_expires_at=NULL,attempts=0,due_at=$3,updated_at=$3 WHERE kind=$1 AND resource_id=$2 AND payload->>'operationId'=$4").bind(JOB_KIND).bind(required(row,"workspace_id")?).bind(now).bind(&a.id).execute(&mut **tx).await?;
        } else if action == Action::Archive {
            a.archived_at_ms = now;
            a.expires_at_ms = now + TTL;
            config["recoveryArchive"] = serde_json::to_value(&a).map_err(|_| fenced())?;
        }
        config["rustLifecycleOperation"] =
            json!({"id":operation,"action":action.as_str(),"phase":"paused"});
        sqlx::query("INSERT INTO zuse_rust_lifecycle_operations(operation_id,workspace_id,command_id,generation,source_machine_id,storage_incarnation_id,action,phase,created_at,updated_at) VALUES($1,$2,$3,$4,$5,$6,$7,'paused',$8,$8)").bind(&operation).bind(required(row,"workspace_id")?).bind(command).bind(generation).bind(required(&op,"source_machine_id")?).bind(required(&op,"storage_incarnation_id")?).bind(action.as_str()).bind(now).execute(&mut **tx).await?;
        sqlx::query("UPDATE api_cloud_workspaces SET state=$2,desired_state=$2,request_config=$3,archive_requested_at=$4,archive_delete_at=$5,status_code=$6,revision=revision+1,updated_at=GREATEST(updated_at+1,$7) WHERE workspace_id=$1")
            .bind(required(row,"workspace_id")?).bind(if action==Action::Archive{"archived"}else{"paused"}).bind(config).bind(if action==Action::Archive{Some(now)}else{None}).bind(if action==Action::Archive{Some(now+zuse_protocol::cloud::ARCHIVE_RETENTION_MS)}else{None}).bind(if action==Action::Archive{"archive-retired"}else{"paused"}).bind(now).execute(&mut **tx).await?;
        Ok(true)
    }
}

fn clock_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// Original observed-runtime telemetry. It never settles provider cost or emits a charge.
pub(super) async fn observe(
    tx: &mut Transaction<'_, Postgres>,
    row: &Value,
    op: &Value,
    machine: Option<&Machine>,
    now: i64,
) -> Result<(), ApiError> {
    if machine.is_some_and(|m| m.state == MachineState::Running) {
        sqlx::query("UPDATE zuse_rust_recovery_operations SET running_since=COALESCE(running_since,$2) WHERE operation_id=$1").bind(required(op,"operation_id")?).bind(now).execute(&mut **tx).await?;
    }
    observe_runtime(
        tx,
        row,
        required(op, "source_machine_id")?,
        op["running_since"].as_i64().unwrap_or(now),
        machine,
        now,
    )
    .await
}
pub(super) async fn observe_runtime(
    tx: &mut Transaction<'_, Postgres>,
    row: &Value,
    source: &str,
    since: i64,
    machine: Option<&Machine>,
    now: i64,
) -> Result<(), ApiError> {
    let running = machine.is_some_and(|m| m.state == MachineState::Running);
    let mut value = json!({"accountId":row["account_id"],"resourceKind":"workspace","resourceId":row["workspace_id"],"provider":row["provider"],"providerSandboxId":source,"observedAtMs":now,"vcpuCount":machine.and_then(|m|m.vcpu).unwrap_or(1),"memoryMib":machine.and_then(|m|m.memory_bytes).unwrap_or(1024*1024*1024)/(1024*1024)});
    if running {
        value["runningSinceMs"] = json!(since);
    }
    sqlx::query("INSERT INTO api_cloud_runtime_observations(provider,provider_sandbox_id,observation) VALUES($1,$2,$3) ON CONFLICT DO NOTHING").bind(required(row,"provider")?).bind(source).bind(&value).execute(&mut **tx).await?;
    let previous:Value=sqlx::query_scalar("SELECT observation FROM api_cloud_runtime_observations WHERE provider=$1 AND provider_sandbox_id=$2 FOR UPDATE").bind(required(row,"provider")?).bind(source).fetch_one(&mut **tx).await?;
    let previous_at = previous["observedAtMs"].as_i64().unwrap_or(now);
    if previous_at >= now {
        return Ok(());
    }
    if running
        && previous["accountId"] == row["account_id"]
        && previous["resourceId"] == row["workspace_id"]
        && previous["resourceKind"] == "workspace"
        && previous["runningSinceMs"].is_i64()
        && now - previous_at <= 120000
    {
        let start = previous_at.max(since);
        if start < now {
            let iso = |t: i64| {
                chrono::DateTime::from_timestamp_millis(t)
                    .map(|d| d.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
                    .unwrap_or_default()
            };
            let event = format!(
                "runtime:{}:{source}:{start}:{now}",
                required(row, "provider")?
            );
            let mut payload = json!({"eventId":event,"accountId":row["account_id"],"eventName":"zuse_cloud_runtime_observed_ms","units":now-start,"occurredAtMs":now,"metadata":{"provider":row["provider"],"resource_kind":"workspace","resource_id":row["workspace_id"],"provider_sandbox_id":source,"started_at":iso(start),"ended_at":iso(now),"vcpu_count":value["vcpuCount"].to_string(),"memory_mib":value["memoryMib"].to_string(),"measurement":"observed","billable":"false"}});
            let periods: bool =
                sqlx::query_scalar("SELECT to_regclass('api_cloud_billing_periods') IS NOT NULL")
                    .fetch_one(&mut **tx)
                    .await?;
            let billing: Option<String> = if periods {
                sqlx::query_scalar("SELECT to_jsonb(p)->>'billing_provider' FROM api_cloud_billing_periods p WHERE account_id=$1 AND period_start<=$2 AND period_end>$2 AND COALESCE(to_jsonb(p)->>'billing_provider','manual')<>'manual' ORDER BY period_start DESC LIMIT 1").bind(required(row,"account_id")?).bind(now).fetch_optional(&mut **tx).await?.flatten()
            } else {
                None
            };
            payload["billingProvider"] = json!(billing.unwrap_or_else(|| "polar".into()));
            sqlx::query("INSERT INTO api_cloud_usage_outbox(event_id,payload,created_at,next_attempt_at) VALUES($1,$2,$3,$3) ON CONFLICT DO NOTHING").bind(event).bind(payload).bind(now).execute(&mut **tx).await?;
        }
    }
    sqlx::query("UPDATE api_cloud_runtime_observations SET observation=$3 WHERE provider=$1 AND provider_sandbox_id=$2").bind(required(row,"provider")?).bind(source).bind(value).execute(&mut **tx).await?;
    Ok(())
}
