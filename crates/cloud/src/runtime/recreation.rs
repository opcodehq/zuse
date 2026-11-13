//! Explicit replacement uses verified recovery, independently of archive policy.
//! This orchestrator never allocates or deletes provider resources directly.
use super::*;
use crate::{
    access::{Access, Permission},
    jobs,
};
use zuse_protocol::cloud::ArchivePhase;
pub(super) mod environment;
mod worker;
pub const MIGRATION: &str = include_str!("../../migrations/0027_computer_recreations.sql");
/// `targetRelease` value asking recreation for the newest compatible base.
pub const TARGET_LATEST: &str = "latest";
const WORKLOAD_LIMIT: &str = "Recreation restores repositories, Zuse data and supported agent session files. Other home files and software installed outside the environment setup are not copied. Record installation commands in Environment setup before recreating.";
fn fenced() -> ApiError {
    ApiError::conflict("computer_recreation_fenced")
}
fn unavailable() -> ApiError {
    ApiError::conflict("computer_recreation_unavailable")
}
/// The deployment's staging pin no longer offers this computer's candidate.
fn candidate_not_selected() -> ApiError {
    ApiError::conflict("staging_managed_base_candidate_not_selected")
}
/// `intent` is the workspace's `rustRecreation`; it carries the target base.
fn public(operation: &Value, intent: &Value) -> Value {
    let target = if intent["operationId"] == operation["operation_id"] {
        intent["targetReleaseId"].clone()
    } else {
        Value::Null
    };
    json!({"operationId":operation["operation_id"],"workspaceId":operation["workspace_id"],"state":operation["phase"],"requiresAttention":operation["phase"]=="attention-required","failedStage":operation["retry_phase"],"errorCode":operation["error_code"],"recoveryVerified":operation["verified_archive_sha256"].is_string(),"createdAt":operation["created_at"],"updatedAt":operation["updated_at"],"targetReleaseId":target,"softwareNotice":WORKLOAD_LIMIT})
}
/// The newer base an "Update computer" recreation moves onto, if any. Server
/// owned; only meaningful while that recreation is active.
pub(super) fn target_release(row: &Value) -> Option<&str> {
    leases_blocked(row)
        .then(|| row["request_config"]["rustRecreation"]["targetReleaseId"].as_str())
        .flatten()
}
/// For a staging-candidate computer, the exact candidate (by hash) its
/// "Update computer" recreation moves onto. Server owned, like the release.
pub(super) fn target_candidate(row: &Value) -> Option<&str> {
    target_release(row)
        .and(row["request_config"]["rustRecreation"]["targetCandidateSha256"].as_str())
}
/// Release-derived workspace flags; mirrors managed creation.
const RELEASE_FLAGS: &[&str] = &[
    "rustGithubCliVersion",
    "rustRecreationVersion",
    "rustRuntimeRepairVersion",
    "rustRuntimeSupervisorVersion",
    "rustToolOutputVersion",
    "rustMemorySleepVersion",
];
const RELEASE_KEYS: &[&str] = &["providerArtifactBinding", "initialRuntimeBootstrap"];
/// A staging candidate's exact identity (`ManagedBaseCandidate::settings()`).
const CANDIDATE_KEYS: &[&str] = &[
    "managedCandidateSha256",
    "managedCandidateArtifactId",
    "managedBaseReadiness",
];
/// Repin a workspace config onto `settings` (a `ManagedBaseRelease::settings()`,
/// or `ManagedBaseCandidate::settings()` for a staging-candidate computer).
pub(super) fn repin_config(config: &mut Value, settings: &Value) -> Result<(), ApiError> {
    let object = config.as_object_mut().ok_or_else(fenced)?;
    for flag in RELEASE_FLAGS {
        if settings[*flag] == 1 {
            object.insert((*flag).into(), json!(1));
        } else {
            object.remove(*flag);
        }
    }
    for key in RELEASE_KEYS {
        if settings[*key].is_null() {
            object.remove(*key);
        } else {
            object.insert((*key).into(), settings[*key].clone());
        }
    }
    // Candidate computers only move between candidates: their identity is
    // replaced exactly, never dropped.
    if object.contains_key("managedCandidateSha256") {
        for key in CANDIDATE_KEYS {
            let value = settings[*key].as_str().ok_or_else(fenced)?;
            object.insert((*key).into(), json!(value));
        }
    }
    let release = settings["managedReleaseId"].as_str().ok_or_else(fenced)?;
    object.insert("managedReleaseId".into(), json!(release));
    // The replacement machine boots the new base's own runtime.
    super::runtime_updates::fresh_machine(config);
    Ok(())
}
pub(crate) fn workspace_metadata(row: &Value) -> Value {
    let config = &row["request_config"];
    let operation = &config["rustRecreation"];
    json!({"computerRecreation":if operation["operationId"].is_string(){json!({"operationId":operation["operationId"],"state":operation["phase"],"requiresAttention":operation["phase"]=="attention-required","errorCode":if operation["phase"]=="attention-required"{row["status_code"].clone()}else{Value::Null},"targetReleaseId":operation["targetReleaseId"],"softwareNotice":WORKLOAD_LIMIT})}else{Value::Null}})
}
/// Config is server-owned. All source identities remain bound while the archive
/// worker uses the explicit paused-computer capture mode.
pub(super) fn capture_intent(row: &Value) -> bool {
    let intent = &row["request_config"]["rustRecreation"];
    row["desired_state"] == "paused"
        && !lifecycle::deleted(row)
        && matches!(intent["phase"].as_str(), Some("capturing" | "retiring"))
        && intent["operationId"]
            .as_str()
            .is_some_and(|id| !id.is_empty())
        && intent["sourceGeneration"] == row["request_config"]["runtimeGeneration"]
        && intent["sourceMachineId"] == row["provider_sandbox_id"]
        && intent["storageIncarnationId"]
            == row["request_config"]["rustRuntimeStorage"]["storageIncarnationId"]
}
/// Any active or attention-required recreation keeps execution closed.
pub(super) fn leases_blocked(row: &Value) -> bool {
    row["request_config"]["rustRecreation"]["operationId"].is_string()
        && !matches!(
            row["request_config"]["rustRecreation"]["phase"].as_str(),
            Some("ready" | "cancelled")
        )
}
// Static predicate, composed only into authority-owned SQL. No request values
// are interpolated. Ordinary archive authorization remains unchanged.
pub(super) const CAPTURE_SQL: &str = "(w.desired_state='archived' OR (w.desired_state='paused' AND w.request_config->'rustRecreation'->>'phase' IN ('capturing','retiring') AND w.request_config->'rustRecreation'->>'sourceGeneration'=w.request_config->>'runtimeGeneration' AND w.request_config->'rustRecreation'->>'sourceMachineId'=w.provider_sandbox_id AND w.request_config->'rustRecreation'->>'storageIncarnationId'=w.request_config->'rustRuntimeStorage'->>'storageIncarnationId'))";
impl RuntimeControl {
    /// Optional release feature: existing staging schemas continue serving
    /// ordinary computers until 0026/0027 are finalized together.
    pub(crate) async fn recreation_available(&self, row: &Value) -> Result<bool, ApiError> {
        if !self.recreation_eligible(row) {
            return Ok(false);
        }
        self.recreation_schema_present().await
    }
    async fn recreation_schema_present(&self) -> Result<bool, ApiError> {
        // Optional migrations are applied with a service restart. Cache both
        // absence and presence; ordinary workers do not poll pg_catalog.
        Ok(*self
            .recreation_schema_ready
            .get_or_try_init(|| async {
                sqlx::query_scalar(
                    "SELECT to_regclass('zuse_rust_computer_recreations') IS NOT NULL",
                )
                .fetch_one(&self.pool)
                .await
                .map_err(ApiError::from)
            })
            .await?)
    }
    pub(crate) fn recreation_eligible(&self, row: &Value) -> bool {
        self.recovery_enabled
            && self.transcript_objects.is_some()
            && self.recovery_allowed_for_recreation(required(row, "workspace_id").unwrap_or(""))
            && !lifecycle::deleted(row)
            && !super::repair::leases_blocked(row)
            && !leases_blocked(row)
            && matches!(row["state"].as_str(), Some("ready" | "paused"))
            && row["request_config"]["rustRecreationVersion"] == 1
            && storage_enrolled(row)
            && row["managed_release_id"].is_string()
            && row["request_config"]["providerAuthMode"] == "broker-v1"
            && row["request_config"]["rustStorageEnrollment"]["capabilities"]
                .as_array()
                .is_some_and(|caps| {
                    caps.iter().any(|cap| cap == recovery::CAPABILITY)
                        && caps
                            .iter()
                            .any(|cap| cap == zuse_protocol::cloud_lifecycle::RECREATION_CAPABILITY)
                        && caps
                            .iter()
                            .any(|cap| cap == zuse_protocol::cloud_lifecycle::CAPABILITY)
                })
    }
    fn recovery_allowed_for_recreation(&self, id: &str) -> bool {
        self.recovery_enabled
            && (self.recovery_workspaces.is_empty()
                || self.recovery_workspaces.iter().any(|known| known == id))
    }
    /// Check the initiating identity outside any provider-effect transaction.
    pub(super) async fn recreation_effect_scope(&self, row: &Value) -> Result<(), ApiError> {
        if !leases_blocked(row) {
            return Ok(());
        }
        if row["request_config"]["rustRecreation"]["phase"] == "attention-required" {
            return Err(ApiError::conflict("computer_recreation_attention_required"));
        }
        let actor = required(&row["request_config"]["rustRecreation"], "actorId")?;
        self.scope_access(required(row, "workspace_id")?, Some(actor), true)
            .await?;
        Ok(())
    }
    async fn recreation_schema(&self) -> Result<(), ApiError> {
        let ready = self.recreation_schema_present().await?;
        if !ready {
            return Err(ApiError::unavailable("computer_recreation_schema_required"));
        }
        Ok(())
    }
    async fn recreation_source(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        row: &Value,
        now: i64,
    ) -> Result<String, ApiError> {
        let release = required(row, "managed_release_id")?;
        if row["request_config"]["managedReleaseId"] != release {
            return Err(fenced());
        }
        if let Some(target) = target_release(row) {
            return Ok(self.chosen_target(tx, row, target, now).await?.artifact_id);
        }
        if row["request_config"]["managedCandidateSha256"].is_string() {
            let source = self.source_candidate(row)?;
            Ok(source.load(tx, now, true).await?.artifact_id)
        } else {
            crate::managed_bases::retained_source(tx, required(row, "provider")?, release).await
        }
    }
    /// The deployment's staging pin, when it governs this computer's owner and
    /// provider.
    fn applicable_pin(
        &self,
        row: &Value,
    ) -> Result<&crate::managed_bases::candidate::StagingBasePin, ApiError> {
        self.staging_candidate
            .as_deref()
            .filter(|pin| {
                pin.applies(
                    row["account_id"].as_str().unwrap_or(""),
                    row["provider"].as_str().unwrap_or(""),
                )
            })
            .ok_or_else(candidate_not_selected)
    }
    /// The exact candidate this computer runs: the current pin, or the
    /// pin's retained binding for it once the pin has moved on.
    fn source_candidate(
        &self,
        row: &Value,
    ) -> Result<crate::managed_bases::candidate::StagingBasePin, ApiError> {
        self.applicable_pin(row)?
            .pin_for_row(row)
            .map_err(|error| match error.code {
                "staging_managed_base_candidate_fenced" => candidate_not_selected(),
                _ => error,
            })
    }
    /// The base an "Update computer" recreation already chose. A candidate
    /// target stays valid only while it is still the deployment's exact pin.
    async fn chosen_target(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        row: &Value,
        target: &str,
        now: i64,
    ) -> Result<crate::managed_bases::RecreationTarget, ApiError> {
        let Some(candidate) = target_candidate(row) else {
            return crate::managed_bases::recreation_target(tx, required(row, "provider")?, target)
                .await;
        };
        let pin = self.applicable_pin(row)?;
        if pin.release_id != target || pin.candidate_sha256 != candidate {
            return Err(candidate_not_selected());
        }
        pin.recreation_target(tx, now).await
    }
    pub async fn recreate_computer(
        &self,
        id: &str,
        access: &Access,
        key: &str,
        target: Option<&str>,
        now: i64,
    ) -> Result<Value, ApiError> {
        if uuid::Uuid::parse_str(key).is_err() {
            return Err(ApiError::bad_request("invalid_recreation_idempotency_key"));
        }
        if target.is_some_and(|target| target != TARGET_LATEST) {
            return Err(ApiError::bad_request("invalid_recreation_target_release"));
        }
        self.recreation_schema().await?;
        self.recovery_ready().await?;
        self.scope_admission(access).await?;
        self.scope_access(id, Some(&access.actor.account_id), true)
            .await?;
        let mut tx = self.pool.begin().await?;
        sqlx::query("SELECT generation FROM zuse_rust_control_owners WHERE resource_kind='workspace' AND resource_id=$1 AND backend='rust' FOR UPDATE").bind(id).fetch_optional(&mut *tx).await?;
        let (row, generation, _) = locked_workspace(&mut tx, id).await?;
        if row["account_id"] != access.owner_id {
            return Err(ApiError::not_found("cloud_workspace_not_found"));
        }
        if access.workspace_permission(&access.owner_id, &row["request_config"])?
            != Permission::Edit
        {
            return Err(ApiError::forbidden("workspace_access_denied"));
        }
        let prior:Option<Value>=sqlx::query_scalar("SELECT to_jsonb(r) FROM zuse_rust_computer_recreations r WHERE workspace_id=$1 AND idempotency_key=$2 FOR UPDATE").bind(id).bind(key).fetch_optional(&mut *tx).await?;
        if let Some(prior) = prior {
            if prior["actor_id"] != access.actor.account_id {
                return Err(ApiError::conflict(
                    "computer_recreation_idempotency_conflict",
                ));
            }
            tx.commit().await?;
            return Ok(public(&prior, &row["request_config"]["rustRecreation"]));
        }
        if !self.recreation_eligible(&row) {
            return Err(unavailable());
        }
        if super::repair::leases_blocked(&row) {
            return Err(ApiError::conflict("runtime_repair_in_progress"));
        }
        let active:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM zuse_rust_computer_recreations WHERE workspace_id=$1 AND phase NOT IN ('ready','cancelled'))").bind(id).fetch_one(&mut *tx).await?;
        if active {
            return Err(ApiError::conflict("computer_recreation_in_progress"));
        }
        if recovery::archive(&row).is_ok_and(|archive| archive.phase != ArchivePhase::Restored) {
            return Err(ApiError::conflict("archive_operation_in_progress"));
        }
        let mut artifact = self.recreation_source(&mut tx, &row, now).await?;
        // "Update computer": restore the verified capture onto the newest
        // compatible base. A staging-candidate computer can only move to the
        // deployment's current candidate pin, the operator's exact selection.
        let mut target_release_id = None;
        let mut target_candidate_sha256 = None;
        if target == Some(TARGET_LATEST) {
            let config = &row["request_config"];
            let needs = crate::managed_bases::RecreationNeeds {
                agent: required(config, "agent")?,
                size: config["computerSizeId"].as_str().unwrap_or("default"),
                organization: required(&row, "account_id")?.starts_with("organization:"),
            };
            if config["managedCandidateSha256"].is_string() {
                let pin = self.applicable_pin(&row)?;
                let source = self.source_candidate(&row)?;
                if let Some(next) = pin
                    .newer_recreation_target(&mut tx, &source, &needs, now)
                    .await?
                {
                    artifact = next.artifact_id;
                    target_release_id = Some(next.release_id);
                    target_candidate_sha256 = Some(pin.candidate_sha256.clone());
                }
            } else {
                let latest = crate::managed_bases::latest_recreation_target(
                    &mut tx,
                    required(&row, "provider")?,
                    required(&row, "managed_release_id")?,
                    &needs,
                )
                .await?;
                if row["managed_release_id"] != latest.release_id.as_str() {
                    artifact = latest.artifact_id;
                    target_release_id = Some(latest.release_id);
                }
            }
        }
        let source = required(&row, "provider_sandbox_id")?;
        let incarnation = required(
            &row["request_config"]["rustRuntimeStorage"],
            "storageIncarnationId",
        )?;
        let shared:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM api_cloud_workspaces WHERE workspace_id<>$1 AND provider=$2 AND provider_sandbox_id=$3 AND state<>'deleted') OR EXISTS(SELECT 1 FROM api_machines m WHERE to_jsonb(m)->>'provider_sandbox_id'=$3 AND state NOT IN ('deleted','destroyed'))").bind(id).bind(required(&row,"provider")?).bind(source).fetch_one(&mut *tx).await?;
        if shared {
            return Err(ApiError::conflict("computer_recreation_source_shared"));
        }
        let phase = row["request_config"]["rustLifecycleOperation"]["phase"].as_str();
        if !phase.is_none_or(|phase| matches!(phase, "ready" | "paused")) {
            return Err(ApiError::conflict("cloud_lifecycle_in_progress"));
        }
        if row["state"] == "paused" {
            let paused:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM zuse_rust_lifecycle_operations WHERE operation_id=$1 AND workspace_id=$2 AND generation=$3 AND source_machine_id=$4 AND storage_incarnation_id=$5 AND phase='paused')").bind(row["request_config"]["rustLifecycleOperation"]["id"].as_str()).bind(id).bind(generation).bind(source).bind(incarnation).fetch_one(&mut *tx).await?;
            if !paused {
                return Err(ApiError::conflict("cloud_lifecycle_quiescence_required"));
            }
        }
        let operation = uuid::Uuid::new_v4().to_string();
        let pause = uuid::Uuid::new_v4().to_string();
        let asleep = row["state"] == "paused";
        let wake = asleep.then(|| uuid::Uuid::new_v4().to_string());
        let phase = if asleep { "waking" } else { "quiescing" };
        sqlx::query("INSERT INTO zuse_rust_lifecycle_operations(operation_id,workspace_id,command_id,generation,source_machine_id,storage_incarnation_id,action,phase,provider_effects,created_at,updated_at) VALUES($1,$2,$3,$4,$5,$6,'pause','quiescing',$7,$8,$8)")
            .bind(&pause).bind(id).bind(format!("recreate:{operation}:pause")).bind(generation).bind(source).bind(incarnation).bind(json!({"recreationFullShutdown":true})).bind(now).execute(&mut *tx).await?;
        if let Some(wake) = &wake {
            lifecycle::require_resume_capacity(
                self.staging_candidate.as_deref(),
                &mut tx,
                &row,
                now,
            )
            .await?;
            sqlx::query("INSERT INTO zuse_rust_lifecycle_operations(operation_id,workspace_id,command_id,generation,source_machine_id,storage_incarnation_id,action,phase,created_at,updated_at) VALUES($1,$2,$3,$4,$5,$6,'resume','waking',$7,$7)")
                .bind(wake).bind(id).bind(format!("recreate:{operation}:wake")).bind(generation).bind(source).bind(incarnation).bind(now).execute(&mut *tx).await?;
        }
        sqlx::query("INSERT INTO zuse_rust_computer_recreations(operation_id,workspace_id,owner_id,actor_id,idempotency_key,provider_id,source_generation,source_machine_id,storage_incarnation_id,managed_release_id,source_artifact_id,candidate_sha256,pause_operation_id,wake_operation_id,phase,due_at,created_at,updated_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$16,$16)")
            .bind(&operation).bind(id).bind(&access.owner_id).bind(&access.actor.account_id).bind(key).bind(required(&row,"provider")?).bind(generation).bind(source).bind(incarnation).bind(required(&row,"managed_release_id")?).bind(&artifact).bind(row["request_config"]["managedCandidateSha256"].as_str()).bind(&pause).bind(wake.as_deref()).bind(phase).bind(now).execute(&mut *tx).await?;
        let mut config = row["request_config"].clone();
        let linked = wake.as_deref().unwrap_or(&pause);
        config["rustLifecycleOperation"] =
            json!({"id":linked,"action":if asleep {"resume"}else{"pause"},"phase":phase});
        config["rustRecreation"] = json!({"operationId":operation,"actorId":access.actor.account_id,"phase":phase,"sourceGeneration":generation,"sourceMachineId":source,"storageIncarnationId":incarnation});
        if let Some(target) = &target_release_id {
            config["rustRecreation"]["targetReleaseId"] = json!(target);
        }
        if let Some(candidate) = &target_candidate_sha256 {
            config["rustRecreation"]["targetCandidateSha256"] = json!(candidate);
        }
        sqlx::query("UPDATE api_cloud_workspaces SET state=$2,desired_state=$3,request_config=$4,status_code=$5,revision=revision+1,updated_at=GREATEST(updated_at+1,$6),next_action_at=$6 WHERE workspace_id=$1")
            .bind(id).bind(if asleep {"resuming"}else{"pausing"}).bind(if asleep {"ready"}else{"paused"}).bind(&config).bind(format!("computer-recreate-{phase}")).bind(now).execute(&mut *tx).await?;
        let job = jobs::enqueue(
            &mut tx,
            &format!("lifecycle:{linked}"),
            lifecycle::JOB_KIND,
            id,
            generation,
            &json!({"operationId":linked}),
            now,
        )
        .await?;
        sqlx::query("UPDATE zuse_rust_jobs SET max_attempts=120 WHERE job_id=$1")
            .bind(job)
            .execute(&mut *tx)
            .await?;
        let operation: Value = sqlx::query_scalar(
            "SELECT to_jsonb(r) FROM zuse_rust_computer_recreations r WHERE operation_id=$1",
        )
        .bind(&operation)
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(public(&operation, &config["rustRecreation"]))
    }
    pub async fn recreation_status(
        &self,
        id: &str,
        operation: &str,
        access: &Access,
    ) -> Result<Value, ApiError> {
        self.recreation_schema().await?;
        self.scope_access(id, Some(&access.actor.account_id), false)
            .await?;
        let row:Value=sqlx::query_scalar("SELECT to_jsonb(w) FROM api_cloud_workspaces w WHERE workspace_id=$1 AND account_id=$2").bind(id).bind(&access.owner_id).fetch_optional(&self.pool).await?.ok_or_else(||ApiError::not_found("cloud_workspace_not_found"))?;
        access.workspace_permission(&access.owner_id, &row["request_config"])?;
        let op:Value=sqlx::query_scalar("SELECT to_jsonb(r) FROM zuse_rust_computer_recreations r WHERE operation_id=$1 AND workspace_id=$2 AND owner_id=$3").bind(operation).bind(id).bind(&access.owner_id).fetch_optional(&self.pool).await?.ok_or_else(||ApiError::not_found("computer_recreation_not_found"))?;
        Ok(public(&op, &row["request_config"]["rustRecreation"]))
    }
}

impl RuntimeControl {
    /// Called under the existing verified-archive retirement admission, before
    /// committing any provider deletion intent. Preserve the exact backup/base
    /// references even if replacement allocation later fails.
    pub(super) async fn recreation_pre_retirement(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        row: &Value,
        archive: &zuse_protocol::cloud::RecoveryArchive,
        now: i64,
    ) -> Result<(), ApiError> {
        let Some(operation) = row["request_config"]["rustRecreation"]["operationId"].as_str()
        else {
            return Ok(());
        };
        if row["request_config"]["rustRecreation"]["phase"] == "ready" {
            return Ok(());
        }
        let op:Value=sqlx::query_scalar("SELECT to_jsonb(r) FROM zuse_rust_computer_recreations r WHERE operation_id=$1 AND workspace_id=$2 FOR UPDATE").bind(operation).bind(required(row,"workspace_id")?).fetch_optional(&mut **tx).await?.ok_or_else(fenced)?;
        if !matches!(op["phase"].as_str(), Some("capturing" | "retiring"))
            || !archive.backup_verified()
            || op["owner_id"] != row["account_id"]
            || op["source_machine_id"] != row["provider_sandbox_id"]
            || op["source_machine_id"] != archive.source_sandbox_id
            || op["source_generation"] != row["request_config"]["runtimeGeneration"]
            || op["storage_incarnation_id"]
                != row["request_config"]["rustRuntimeStorage"]["storageIncarnationId"]
            || op["managed_release_id"] != row["managed_release_id"]
            || (!op["archive_operation_id"].is_null() && op["archive_operation_id"] != archive.id)
        {
            return Err(fenced());
        }
        // A full guest shutdown is a separate receipt from memory parking.
        // Bind the final retirement admission to that exact linked pause.
        let full_pause:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM zuse_rust_lifecycle_operations WHERE operation_id=$1 AND workspace_id=$2 AND generation=$3 AND source_machine_id=$4 AND storage_incarnation_id=$5 AND action='pause' AND phase='paused' AND provider_effects->>'recreationFullShutdown'='true')")
            .bind(op["pause_operation_id"].as_str()).bind(required(row,"workspace_id")?).bind(op["source_generation"].as_i64()).bind(op["source_machine_id"].as_str()).bind(op["storage_incarnation_id"].as_str()).fetch_one(&mut **tx).await?;
        if !full_pause
            || row["request_config"]["rustLifecycleOperation"]["id"] != op["pause_operation_id"]
            || row["request_config"]["rustMemoryParked"]["operationId"] == op["pause_operation_id"]
        {
            return Err(fenced());
        }
        let artifact = self.recreation_source(tx, row, now).await?;
        if artifact != op["source_artifact_id"] {
            return Err(fenced());
        }
        let manifest = archive.manifest.as_ref().ok_or_else(fenced)?;
        sqlx::query("UPDATE zuse_rust_computer_recreations SET archive_operation_id=$2,verified_archive_sha256=$3 WHERE operation_id=$1").bind(operation).bind(&archive.id).bind(&manifest.archive_sha256).execute(&mut **tx).await?;
        Ok(())
    }
    pub async fn retry_recreation(
        &self,
        id: &str,
        operation: &str,
        access: &Access,
        now: i64,
    ) -> Result<Value, ApiError> {
        self.scope_admission(access).await?;
        self.scope_access(id, Some(&access.actor.account_id), true)
            .await?;
        let mut tx = self.pool.begin().await?;
        sqlx::query("SELECT generation FROM zuse_rust_control_owners WHERE resource_kind='workspace' AND resource_id=$1 AND backend='rust' FOR UPDATE").bind(id).fetch_optional(&mut *tx).await?;
        let (row, _, _) = locked_workspace(&mut tx, id).await?;
        if row["account_id"] != access.owner_id
            || access.workspace_permission(&access.owner_id, &row["request_config"])?
                != Permission::Edit
        {
            return Err(ApiError::forbidden("workspace_access_denied"));
        }
        let op:Value=sqlx::query_scalar("SELECT to_jsonb(r) FROM zuse_rust_computer_recreations r WHERE operation_id=$1 AND workspace_id=$2 AND owner_id=$3 FOR UPDATE").bind(operation).bind(id).bind(&access.owner_id).fetch_optional(&mut *tx).await?.ok_or_else(||ApiError::not_found("computer_recreation_not_found"))?;
        if op["phase"] != "attention-required" {
            tx.commit().await?;
            return Ok(public(&op, &row["request_config"]["rustRecreation"]));
        }
        if op["actor_id"] != access.actor.account_id {
            return Err(ApiError::forbidden("computer_recreation_requestor_changed"));
        }
        let next = op["retry_phase"].as_str().ok_or_else(fenced)?;
        if row["request_config"]["rustRecreation"]["operationId"] != operation
            || lifecycle::deleted(&row)
        {
            return Err(fenced());
        }
        self.recreation_source(&mut tx, &row, now).await?;
        sqlx::query("UPDATE zuse_rust_jobs SET state='pending',attempts=0,due_at=$2,last_error_code=NULL,updated_at=$2 WHERE resource_id=$1 AND state IN ('failed','pending') AND lease_token IS NULL AND payload->>'operationId'=ANY($3)").bind(id).bind(now).bind(vec![op["wake_operation_id"].as_str().unwrap_or("").to_owned(),op["pause_operation_id"].as_str().unwrap_or("").to_owned(),op["archive_operation_id"].as_str().unwrap_or("").to_owned(),op["restore_operation_id"].as_str().unwrap_or("").to_owned()]).execute(&mut *tx).await?;
        worker::transition(&mut tx, &row, &op, next, now).await?;
        let current: Value = sqlx::query_scalar(
            "SELECT to_jsonb(r) FROM zuse_rust_computer_recreations r WHERE operation_id=$1",
        )
        .bind(operation)
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(public(&current, &row["request_config"]["rustRecreation"]))
    }
}

#[cfg(test)]
mod tests;
