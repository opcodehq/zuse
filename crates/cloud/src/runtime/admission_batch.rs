//! Atomic fresh-admission reads and writes, with existing Rust policy validation.
//!
//! The caller
//! must first retain the same transaction's authoritative scope/idempotency,
//! project, connection, billing, snapshot and0023 base locks; validate the read
//! snapshot with the existing creation rules; and encrypt the first message.
//! Existing/retained workspace admission must keep its normal reconciler.
use super::*;
use serde::Serialize;

/// Internal prepared data only: no Deserialize/Debug and no plaintext prompt or
/// provider credential. Its constructor belongs beside the existing validated
/// creation config builder when the prototype is integrated after live canary.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct FreshAdmissionWrite {
    pub workspace_id: String,
    pub owner_id: String,
    pub project_id: String,
    pub build_id: Option<String>,
    pub managed_release_id: Option<String>,
    pub provider: String,
    pub chat_id: String,
    pub session_id: String,
    pub branch: String,
    pub base_ref: String,
    pub wrapped_transcript_key: String,
    pub idempotency_key: String,
    pub request_config: Value,
    pub command_id: String,
    pub turn_id: String,
    pub launch_ciphertext: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub initial_task_command: Option<super::task_commands::initial::InitialPrepared>,
    pub admission_ciphertext: String,
    pub launch_expires_at: i64,
    pub operation_id: String,
    pub snapshot_id: String,
    pub machine_name: String,
    pub storage_incarnation_id: String,
    pub sealed_boot_token: String,
    pub boot_token_hash: String,
    pub boot_expires_at: i64,
    pub job_id: String,
    pub now: i64,
}
impl FreshAdmissionWrite {
    fn validate_shape(&self) -> Result<(), ApiError> {
        let config = &self.request_config;
        if self.workspace_id.is_empty()
            || self.owner_id.is_empty()
            || self.project_id.is_empty()
            || self.build_id.is_none() && self.managed_release_id.is_none()
            || config["runtimeGeneration"] != 1
            || config["gatewayEpoch"] != 1
            || config["runtimeRpcProtocol"] != zuse_protocol::cloud_gateway::RUST_RPC
            || config["rustAllocationOperationId"] != self.operation_id
            || config["rustRuntimeStorage"]["storageIncarnationId"] != self.storage_incarnation_id
            || !config["rustStorageEnrollment"].is_null()
            || !config["runtimeBootstrapReceipt"].is_null()
            || !config["recoveryArchive"].is_null()
            || self.snapshot_id.is_empty()
            || self.launch_expires_at <= self.now
            || self.boot_expires_at <= self.now
            || !super::scope::native_organization_task(
                &json!({"account_id":self.owner_id,"request_config":config}),
            )
        {
            return Err(ApiError::conflict("fresh_admission_plan_invalid"));
        }
        Ok(())
    }
}

// Data-modifying CTE dependencies are explicit: no sibling insert relies on an
// unspecified evaluation order. All encrypted data and IDs are bound parameters,
// never interpolated into SQL. The workspace is inserted with the final original
// allocation identity once, eliminating a re-read and separate UPDATE.
const WRITE: &str = r#"
WITH input AS MATERIALIZED(SELECT $1::jsonb AS j),
workspace AS (
 INSERT INTO api_cloud_workspaces(workspace_id,account_id,project_id,build_id,managed_release_id,provider,
 runtime_state,chat_id,initial_session_id,branch,base_ref,state,desired_state,status_code,
 wrapped_transcript_key,idempotency_key,request_config,runtime_boot_token_hash,runtime_boot_token_expires_at,
 next_action_at,revision,created_at,updated_at,last_activity_at)
 SELECT j->>'workspaceId',j->>'ownerId',j->>'projectId',j->>'buildId',j->>'managedReleaseId',j->>'provider',
 'offline',j->>'chatId',j->>'sessionId',j->>'branch',j->>'baseRef','provisioning','ready','allocating',
 j->>'wrappedTranscriptKey',j->>'idempotencyKey',j->'requestConfig',j->>'bootTokenHash',(j->>'bootExpiresAt')::bigint,
 (j->>'now')::bigint,1,(j->>'now')::bigint,(j->>'now')::bigint+1,(j->>'now')::bigint FROM input
 RETURNING *
),
launch AS (
 INSERT INTO api_cloud_workspace_launch_intents(workspace_id,account_id,chat_id,session_id,turn_id,command_id,ciphertext,expires_at,created_at)
 SELECT w.workspace_id,w.account_id,w.chat_id,w.initial_session_id,j->>'turnId',j->>'commandId',j->>'launchCiphertext',
 (j->>'launchExpiresAt')::bigint,(j->>'now')::bigint FROM workspace w CROSS JOIN input RETURNING workspace_id
),
history AS (
 INSERT INTO zuse_rust_task_admissions(workspace_id,session_id,ciphertext,created_at)
 SELECT w.workspace_id,w.initial_session_id,j->>'admissionCiphertext',(j->>'now')::bigint
 FROM workspace w JOIN launch l USING(workspace_id) CROSS JOIN input RETURNING workspace_id
),
owner AS (
 INSERT INTO zuse_rust_control_owners(resource_kind,resource_id,backend,generation,updated_at)
 SELECT 'workspace',w.workspace_id,'rust',1,(j->>'now')::bigint
 FROM workspace w JOIN history h USING(workspace_id) CROSS JOIN input RETURNING resource_id
),
allocation AS (
 INSERT INTO zuse_rust_allocations(operation_id,workspace_id,generation,snapshot_id,machine_name,
 storage_incarnation_id,sealed_boot_token,boot_expires_at,phase,created_at,updated_at)
 SELECT j->>'operationId',w.workspace_id,1,j->>'snapshotId',j->>'machineName',j->>'storageIncarnationId',
 j->>'sealedBootToken',(j->>'bootExpiresAt')::bigint,'planned',(j->>'now')::bigint,(j->>'now')::bigint
 FROM workspace w JOIN owner o ON o.resource_id=w.workspace_id CROSS JOIN input RETURNING workspace_id,operation_id
),
job AS (
 INSERT INTO zuse_rust_jobs(job_id,idempotency_key,kind,resource_id,resource_generation,payload,due_at,max_attempts,created_at,updated_at)
 SELECT j->>'jobId','allocation:'||a.operation_id,'workspace-allocate',a.workspace_id,1,
 jsonb_build_object('operationId',a.operation_id),(j->>'now')::bigint,120,(j->>'now')::bigint,(j->>'now')::bigint
 FROM allocation a CROSS JOIN input RETURNING resource_id
)
SELECT to_jsonb(w) FROM workspace w JOIN job ON job.resource_id=w.workspace_id
"#;

// Separate statement: an ordinary 0025 deployment never parses references to
// the optional 0026 tables. New v2 admission retains one atomic write execution.
static WRITE_V2: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
    let prefix = WRITE
        .strip_suffix(
            "SELECT to_jsonb(w) FROM workspace w JOIN job ON job.resource_id=w.workspace_id\n",
        )
        .expect("static admission write suffix");
    format!(
        "{prefix}{}",
        r#",
task_input AS MATERIALIZED(SELECT j,j->'initialTaskCommand' AS p FROM input),
task_policy AS (
 INSERT INTO zuse_rust_task_command_policies(workspace_id,owner_id,policy_revision,ciphertext,created_at,updated_at)
 SELECT w.workspace_id,w.account_id,1,p->>'policyCiphertext',(j->>'now')::bigint,(j->>'now')::bigint
 FROM workspace w JOIN owner o ON o.resource_id=w.workspace_id CROSS JOIN task_input RETURNING workspace_id
),
task_session AS (
 INSERT INTO zuse_rust_task_command_sessions(workspace_id,session_id,thread_id,state,metadata_ciphertext,created_at)
 SELECT w.workspace_id,w.initial_session_id,w.chat_id,'active',p->>'sessionCiphertext',(j->>'now')::bigint
 FROM workspace w JOIN task_policy USING(workspace_id) CROSS JOIN task_input RETURNING workspace_id,session_id
),
task_mailbox AS (
 INSERT INTO zuse_rust_mailboxes(workspace_id,sequence,revision)
 SELECT s.workspace_id,CASE WHEN jsonb_typeof(p->'command')='object' THEN 1 ELSE 0 END,
 CASE WHEN jsonb_typeof(p->'command')='object' THEN 1 ELSE 0 END
 FROM task_session s CROSS JOIN task_input RETURNING workspace_id
),
task_envelope AS (
 INSERT INTO zuse_rust_mailbox_commands(workspace_id,command_id,workspace_sequence,revision,lane,fingerprint,envelope,envelope_bytes,state,accepted_at,updated_at)
 SELECT w.workspace_id,j->>'commandId',1,1,'task-execution:'||w.initial_session_id,
 p#>>'{command,envelope,fingerprint}',p#>'{command,envelope}',(p#>>'{command,envelopeBytes}')::bigint,
 'waiting-for-runtime',(j->>'now')::bigint,(j->>'now')::bigint
 FROM workspace w JOIN task_mailbox USING(workspace_id) CROSS JOIN task_input
 WHERE jsonb_typeof(p->'command')='object' RETURNING workspace_id,command_id
),
task_action AS (
 INSERT INTO zuse_rust_task_command_actions(workspace_id,command_id,session_id,actor_id,request_digest,action_kind,acceptance,created_at)
 SELECT w.workspace_id,e.command_id,w.initial_session_id,p->>'actorId',p#>>'{command,requestDigest}','submit',p#>'{command,acceptance}',(j->>'now')::bigint
 FROM workspace w JOIN task_envelope e USING(workspace_id) CROSS JOIN task_input RETURNING workspace_id,command_id
),
task_execution AS (
 INSERT INTO zuse_rust_task_command_execution(workspace_id,command_id,session_id,message_id,fingerprint,state,presentation_ciphertext,created_at,updated_at)
 SELECT w.workspace_id,a.command_id,w.initial_session_id,j->>'turnId',p#>>'{command,envelope,fingerprint}','queued',p#>>'{command,presentationCiphertext}',(j->>'now')::bigint,(j->>'now')::bigint
 FROM workspace w JOIN task_action a USING(workspace_id) CROSS JOIN task_input RETURNING workspace_id
)
SELECT to_jsonb(w) FROM workspace w JOIN job ON job.resource_id=w.workspace_id
 JOIN task_session s USING(workspace_id) CROSS JOIN task_input
 WHERE p->'command'='null'::jsonb OR EXISTS(SELECT 1 FROM task_execution e WHERE e.workspace_id=w.workspace_id)
"#
    )
});

impl RuntimeControl {
    /// Fresh-only write phase after the authoritative admission read and validation.
    #[tracing::instrument(name="cloud_admission_stage", skip_all, fields(owner=%plan.owner_id, operation="admission_write"))]
    pub(super) async fn write_fresh_admission_batch_tx(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        plan: &FreshAdmissionWrite,
    ) -> Result<Value, ApiError> {
        plan.validate_shape()?;
        if (plan.request_config["commandDeliveryMode"] == zuse_protocol::cloud_task_commands::MODE)
            != plan.initial_task_command.is_some()
        {
            return Err(ApiError::conflict("fresh_admission_plan_invalid"));
        }
        let value = serde_json::to_value(plan)
            .map_err(|_| ApiError::bad_request("fresh_admission_plan_invalid"))?;
        let statement: &str = if plan.initial_task_command.is_some() {
            WRITE_V2.as_str()
        } else {
            WRITE
        };
        Ok(sqlx::query_scalar(statement)
            .bind(value)
            .fetch_one(&mut **tx)
            .await?)
    }
}

/// Facts read under the same transaction's account/snapshot/project/base locks.
/// Private construction prevents an HTTP payload from impersonating a read permit.
pub(super) struct AdmissionRead(Value);
impl AdmissionRead {
    pub(super) fn get(&self, key: &str) -> &Value {
        &self.0[key]
    }
    pub(super) fn optional(&self, key: &str) -> Option<Value> {
        self.0.get(key).filter(|v| !v.is_null()).cloned()
    }
}
impl RuntimeControl {
    pub(crate) async fn admission_batch_available(&self) -> Result<bool, ApiError> {
        let value=self.admission_batch_schema.get_or_try_init(||async{
            let ready:bool=sqlx::query_scalar("SELECT to_regprocedure('zuse_rust_admission_read_batch(jsonb)') IS NOT NULL AND coalesce(has_function_privilege(to_regprocedure('zuse_rust_admission_read_batch(jsonb)'),'EXECUTE'),false)").fetch_one(&self.pool).await?;
            Ok::<bool,ApiError>(ready)
        }).await?;
        Ok(*value)
    }
    #[tracing::instrument(name="cloud_admission_stage", skip_all, fields(owner=%access.owner_id, operation="admission_read"))]
    pub(super) async fn read_admission_batch_tx(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        access: &crate::access::Access,
        body: &creation::CreateWorkspace,
        now: i64,
    ) -> Result<AdmissionRead, ApiError> {
        let owner = &access.owner_id;
        let pin = self
            .staging_candidate
            .as_ref()
            .filter(|p| p.applies(owner, &body.provider_id));
        let input = json!({"owner":owner,"actor":access.actor.account_id,"now":now,
            "idempotencyKey":body.idempotency_key,"deleteDigest":self.delete_idempotency(owner,&body.idempotency_key),
            "launchDigest":self.launch_idempotency(access,&body.idempotency_key),"projectId":body.project_id,
            "provider":body.provider_id,"connectionId":body.agent_connection_id,
            "agent":if matches!(body.agent.as_str(),"claude"|"claude-code"){"claude"}else{&body.agent},
            "secretBindings":body.secret_bindings.as_deref().unwrap_or_default(),"assetIds":body.attachments.iter().map(|a|&a.asset_id).collect::<Vec<_>>(),"environmentRevision":body.environment_revision,"candidateRelease":pin.map(|p|&p.release_id),
            "stagingAllowance":pin.is_some_and(|p|p.allow_staging_compute),"branch":body.branch,
            "workspaceId":format!("workspace_{}",uuid::Uuid::new_v4().simple())});
        let facts = sqlx::query_scalar("SELECT zuse_rust_admission_read_batch($1)")
            .bind(input)
            .fetch_one(&mut **tx)
            .await?;
        Ok(AdmissionRead(facts))
    }
    pub(super) fn fresh_admission_plan(
        &self,
        mut plan: FreshAdmissionWrite,
        admission: &Value,
    ) -> Result<FreshAdmissionWrite, ApiError> {
        use zeroize::Zeroizing;
        let operation = uuid::Uuid::new_v4().to_string();
        let incarnation = uuid::Uuid::new_v4().to_string();
        let name = format!(
            "zr-{}",
            hash(&format!("{}\n1\n{operation}", plan.workspace_id))
        );
        let boot = Zeroizing::new(format!(
            "rust_boot_{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        ));
        plan.sealed_boot_token = self
            .master
            .seal_api(
                &format!(
                    "rust-workspace-allocation-boot:{}:1:{operation}",
                    plan.workspace_id
                ),
                &boot,
            )
            .map_err(|_| ApiError::unavailable("cloud_boot_grant_unavailable"))?;
        plan.boot_token_hash = hash(&boot);
        plan.boot_expires_at = plan.now + 30 * 60 * 1000;
        plan.operation_id = operation.clone();
        plan.storage_incarnation_id = incarnation.clone();
        plan.machine_name = name[..63].to_owned();
        plan.job_id = uuid::Uuid::new_v4().to_string();
        plan.admission_ciphertext = self
            .master
            .seal_api(
                &format!("task-admission:{}:{}", plan.owner_id, plan.workspace_id),
                &admission.to_string(),
            )
            .map_err(|_| ApiError::unavailable("task_admission_encryption_failed"))?;
        let config = &mut plan.request_config;
        config["rustRuntimeStorage"] =
            json!({"format":"zuse-rust-engine-v1","storageIncarnationId":incarnation});
        config["rustSessionCommandVersion"] = json!(1);
        config["rustAllocationOperationId"] = json!(operation);
        plan.validate_shape()?;
        Ok(plan)
    }
}

#[cfg(test)]
mod tests;
