//! Preparation facts may arrive before storage commitment; they never admit agent execution.
use super::*;
use zuse_protocol::cloud_computers::{
    PreparationStage, PreparationStepState, RuntimePreparationUpdate,
};

fn apply(
    config: &mut Value,
    update: &RuntimePreparationUpdate,
    now: i64,
) -> Result<Option<&'static str>, ApiError> {
    if update.log.as_ref().is_some_and(|log| {
        update.stage != PreparationStage::Environment
            || update.state == PreparationStepState::Running
            || log.stdout.len() > zuse_protocol::cloud_computers::MAX_PREPARATION_LOG_BYTES
            || log.stderr.len() > zuse_protocol::cloud_computers::MAX_PREPARATION_LOG_BYTES
    }) {
        return Err(ApiError::bad_request("invalid_preparation_log"));
    }
    let (stage, running, failure) = match update.stage {
        PreparationStage::Repository => (
            "repository",
            "repository-syncing",
            "repository_preparation_failed",
        ),
        PreparationStage::Environment => (
            "environment",
            "environment-setup",
            "environment_setup_failed",
        ),
        PreparationStage::Credentials => (
            "credentials",
            "credentials-preparing",
            "credential_preparation_failed",
        ),
        _ => return Err(ApiError::bad_request("invalid_runtime_preparation_stage")),
    };
    if update.operation_id.is_empty()
        || update.operation_id.len() > 200
        || config["rustAllocationOperationId"] != update.operation_id
        || config["rustPreparationVersion"] != 1
    {
        return Err(ApiError::conflict("runtime_preparation_operation_fenced"));
    }
    if (update.state == PreparationStepState::Failed
        && update.error_code.as_deref() != Some(failure))
        || (update.state != PreparationStepState::Failed && update.error_code.is_some())
    {
        return Err(ApiError::bad_request("invalid_runtime_preparation_error"));
    }
    let repository_complete = update.stage == PreparationStage::Repository
        && update.state == PreparationStepState::Completed;
    if repository_complete != update.source_commit.is_some()
        || update.source_commit.as_ref().is_some_and(|s| {
            !matches!(s.len(), 40 | 64) || !s.bytes().all(|b| b.is_ascii_hexdigit())
        })
    {
        return Err(ApiError::bad_request("invalid_runtime_preparation_commit"));
    }
    if let Some(commit) = &update.source_commit {
        if config["resolvedSourceCommit"]
            .as_str()
            .is_some_and(|old| !old.eq_ignore_ascii_case(commit))
        {
            return Err(ApiError::conflict("runtime_repository_source_conflict"));
        }
    }
    let prior = &config["preparation"][stage];
    if prior["state"] == "completed" {
        // A restarted process can rediscover a completed local journal. Starting
        // that stage again does not reset its receipt or its resolved commit.
        if update.state == PreparationStepState::Running
            || (update.state == PreparationStepState::Completed
                && prior["sourceCommit"] == json!(update.source_commit))
        {
            return Ok(None);
        }
        return Err(ApiError::conflict("runtime_preparation_stage_completed"));
    }
    let state = match update.state {
        PreparationStepState::Running => "running",
        PreparationStepState::Completed => "completed",
        PreparationStepState::Failed => "failed",
    };
    if prior["state"] == state
        && prior["sourceCommit"] == json!(update.source_commit)
        && prior["errorCode"] == json!(update.error_code)
    {
        return Ok(None);
    }
    let has_observed_start = prior["state"] == "running"
        && prior["startedAt"]
            .as_i64()
            .is_some_and(|start| start <= now);
    let started = if prior["state"] == "running" {
        prior["startedAt"].as_i64().unwrap_or(now)
    } else {
        now
    };
    if !config["preparation"].is_object() {
        config["preparation"] = json!({});
    }
    config["preparation"][stage] = json!({"operationId":update.operation_id,"state":state,"sourceCommit":update.source_commit,"errorCode":update.error_code,"startedAt":started,"updatedAt":now});
    // These are server-received stage boundaries, not isolated Git/setup CPU
    // timers. A terminal report without Running has no measured duration.
    if !config["startupTimings"].is_object() {
        config["startupTimings"] = json!({});
    }
    if !config["startupTimings"]["preparationPhases"].is_object() {
        config["startupTimings"]["preparationPhases"] = json!({});
    }
    let mut observation =
        json!({"clock":"server-receipt-wall","state":state,"lastObservedAtMs":now});
    if update.state == PreparationStepState::Running {
        observation["startedAtMs"] = json!(started);
    } else if has_observed_start {
        observation["startedAtMs"] = json!(started);
        observation["observedDurationMs"] = json!(now.saturating_sub(started));
    }
    config["startupTimings"]["preparationPhases"][stage] = observation;

    if let Some(commit) = &update.source_commit {
        config["resolvedSourceCommit"] = json!(commit.to_ascii_lowercase());
    }
    // Parallel credential reports must never mask a checkout/setup failure.
    // Keep every individual receipt, while the summary describes the blocking
    // failure (checkout first) or still-running work rather than the last reply.
    for (key, code) in [
        ("repository", "repository_preparation_failed"),
        ("environment", "environment_setup_failed"),
        ("credentials", "credential_preparation_failed"),
    ] {
        if config["preparation"][key]["state"] == "failed" {
            return Ok(Some(code));
        }
    }
    for (key, code) in [
        ("repository", "repository-syncing"),
        ("environment", "environment-setup"),
        ("credentials", "credentials-preparing"),
    ] {
        if config["preparation"][key]["state"] == "running" {
            return Ok(Some(code));
        }
    }
    Ok(Some(if update.state == PreparationStepState::Failed {
        failure
    } else {
        running
    }))
}

impl RuntimeControl {
    pub(super) fn early_agent_ready(row: &Value) -> bool {
        let c = &row["request_config"];
        c["rustFastStartVersion"] == 1
            && c["rustAgentPreparationReady"] == true
            && c["rustPreparationIngress"]["operationId"] == c["rustAllocationOperationId"]
            && c["rustPreparationIngress"]["generation"] == c["runtimeGeneration"]
            && c["rustPreparationIngress"]["sourceMachineId"] == row["provider_sandbox_id"]
            && c["rustPreparationIngress"]["storageIncarnationId"]
                == c["rustRuntimeStorage"]["storageIncarnationId"]
            && c["runtimeBootstrapReceipt"]["capabilities"]
                .as_array()
                .is_some_and(|caps| caps.iter().any(|cap| cap == "rust-fast-start-v1"))
            && ["repository", "environment", "credentials"]
                .iter()
                .all(|stage| c["preparation"][stage]["state"] != "failed")
    }
    /// This receipt opens diagnostics only; it never admits an agent command,
    /// completes allocation, or changes the workspace to ready.
    pub async fn preparation_ingress(
        &self,
        id: &str,
        credential: &str,
        operation: &str,
        now: i64,
    ) -> Result<Value, ApiError> {
        self.runtime_scope_access(id, credential, now).await?;
        let mut tx = self.pool.begin().await?;
        let (row, generation, _) = locked_workspace(&mut tx, id).await?;
        super::mailbox::runtime_auth(&row, credential, now)?;
        let mut config = row["request_config"].clone();
        if !storage_enrolled(&row)
            || row["desired_state"] != "ready"
            || blocked(&row)
            || config["rustAllocationOperationId"] != operation
            || operation.is_empty()
            || config["runtimeRpcProtocol"] != zuse_protocol::cloud_gateway::RUST_RPC
        {
            return Err(ApiError::conflict("runtime_preparation_ingress_fenced"));
        }
        config["rustPreparationIngress"] = json!({"version":1,"operationId":operation,"generation":generation,"sourceMachineId":row["provider_sandbox_id"],"storageIncarnationId":config["rustRuntimeStorage"]["storageIncarnationId"],"connectedAt":now});
        sqlx::query("UPDATE api_cloud_workspaces SET request_config=$2,revision=revision+1,updated_at=GREATEST(updated_at+1,$3) WHERE workspace_id=$1").bind(id).bind(config).bind(now).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(json!({"diagnosticsReady":true,"agentReady":false,"credentialPreparationVersion":1}))
    }
    #[tracing::instrument(name="cloud_runtime_request", skip_all, fields(workspace=%id, operation="preparation_update"))]
    pub async fn preparation_update(
        &self,
        id: &str,
        credential: &str,
        update: RuntimePreparationUpdate,
        now: i64,
    ) -> Result<Value, ApiError> {
        let mut tx = self.pool.begin().await?;
        let (row, _, _) = locked_workspace(&mut tx, id).await?;
        if !enrollment_matches(&row)
            || blocked(&row)
            || row["desired_state"] != "ready"
            || row["runtime_credential_hash"]
                .as_str()
                .is_none_or(|h| !equal(h, &hash(credential)))
            || row["request_config"]["runtimeCredentialExpiresAtMs"]
                .as_i64()
                .unwrap_or(0)
                <= now
        {
            return Err(ApiError::unauthorized("workspace_runtime_fenced"));
        }
        let mut config = row["request_config"].clone();
        let status = apply(&mut config, &update, now)?;
        if let Some(log) = &update.log {
            let mut log = log.clone();
            if credential.len() >= 4 {
                log.stdout = log.stdout.replace(credential, "[redacted]");
                log.stderr = log.stderr.replace(credential, "[redacted]");
            }
            let available: bool =
                sqlx::query_scalar("SELECT to_regclass('zuse_rust_preparation_logs') IS NOT NULL")
                    .fetch_one(&mut *tx)
                    .await?;
            if available {
                let owner = row["account_id"]
                    .as_str()
                    .ok_or_else(|| ApiError::unavailable("preparation_log_owner_missing"))?;
                let context = format!("preparation-log:{owner}:{id}:{}", update.operation_id);
                let ciphertext = self
                    .master
                    .seal_api(
                        &context,
                        &serde_json::to_string(&log)
                            .map_err(|_| ApiError::bad_request("invalid_preparation_log"))?,
                    )
                    .map_err(|_| ApiError::unavailable("preparation_log_encryption_failed"))?;
                sqlx::query("INSERT INTO zuse_rust_preparation_logs(workspace_id,operation_id,ciphertext,updated_at) VALUES($1,$2,$3,$4) ON CONFLICT(workspace_id,operation_id) DO UPDATE SET ciphertext=EXCLUDED.ciphertext,updated_at=EXCLUDED.updated_at")
                    .bind(id).bind(&update.operation_id).bind(ciphertext).bind(now).execute(&mut *tx).await?;
            }
        }
        let Some(status) = status else {
            tx.commit().await?;
            return Ok(json!({"applied":false}));
        };
        // Never let a late preparation report replace ready/lifecycle status.
        sqlx::query("UPDATE api_cloud_workspaces SET request_config=$2,status_code=CASE WHEN state IN ('queued','provisioning','setup') AND (runtime_state<>'online' OR request_config->>'rustAgentPreparationReady'='true') THEN $3 ELSE status_code END,revision=revision+1,updated_at=GREATEST(updated_at+1,$4) WHERE workspace_id=$1")
            .bind(id).bind(config).bind(status).bind(now).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(json!({"applied":true}))
    }

    pub async fn preparation_logs(
        &self,
        access: &crate::access::Access,
        id: &str,
    ) -> Result<Value, ApiError> {
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
        let available: bool =
            sqlx::query_scalar("SELECT to_regclass('zuse_rust_preparation_logs') IS NOT NULL")
                .fetch_one(&mut *tx)
                .await?;
        if !available {
            return Err(ApiError::unavailable("preparation_log_schema_not_ready"));
        }
        let operation = row["request_config"]["rustAllocationOperationId"]
            .as_str()
            .unwrap_or_default();
        let ciphertext:Option<String>=sqlx::query_scalar("SELECT ciphertext FROM zuse_rust_preparation_logs WHERE workspace_id=$1 AND operation_id=$2")
            .bind(id).bind(operation).fetch_optional(&mut *tx).await?;
        if let Some(ciphertext) = ciphertext {
            let plain = self
                .master
                .open_api(
                    &format!("preparation-log:{}:{id}:{operation}", access.owner_id),
                    &ciphertext,
                )
                .map_err(|_| ApiError::unavailable("preparation_log_invalid"))?;
            let log: zuse_protocol::cloud_computers::PreparationLog =
                serde_json::from_slice(&plain)
                    .map_err(|_| ApiError::unavailable("preparation_log_invalid"))?;
            return Ok(json!({"operationId":operation,"log":log}));
        }
        Ok(json!({"operationId":operation,"log":null}))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parallel_credentials_preserve_repository_progress_failure_and_operation_fences() {
        let mut config =
            json!({"rustAllocationOperationId":"operation","rustPreparationVersion":1});
        let mut repository = RuntimePreparationUpdate {
            operation_id: "operation".into(),
            stage: PreparationStage::Repository,
            state: PreparationStepState::Running,
            source_commit: None,
            error_code: None,
            log: None,
        };
        let mut credentials = repository.clone();
        credentials.stage = PreparationStage::Credentials;
        apply(&mut config, &repository, 10).unwrap();
        assert_eq!(
            apply(&mut config, &credentials, 11).unwrap(),
            Some("repository-syncing")
        );
        credentials.state = PreparationStepState::Completed;
        assert_eq!(
            apply(&mut config, &credentials, 15).unwrap(),
            Some("repository-syncing")
        );
        assert_eq!(
            config["startupTimings"]["preparationPhases"]["credentials"]["observedDurationMs"],
            4
        );
        assert_eq!(config["preparation"]["repository"]["state"], "running");
        assert!(config["preparation"].get("agent").is_none());
        repository.state = PreparationStepState::Failed;
        repository.error_code = Some("repository_preparation_failed".into());
        assert_eq!(
            apply(&mut config, &repository, 20).unwrap(),
            Some("repository_preparation_failed")
        );
        // A late report from another concurrent stage cannot hide Git failure.
        credentials.stage = PreparationStage::Environment;
        credentials.state = PreparationStepState::Running;
        assert_eq!(
            apply(&mut config, &credentials, 21).unwrap(),
            Some("repository_preparation_failed")
        );
        credentials.stage = PreparationStage::Credentials;
        credentials.operation_id = "different-operation".into();
        assert_eq!(
            apply(&mut config, &credentials, 22).unwrap_err().code,
            "runtime_preparation_operation_fenced"
        );
    }

    #[test]
    fn credential_failure_is_allowlisted_and_survives_late_checkout_completion() {
        let mut config =
            json!({"rustAllocationOperationId":"operation","rustPreparationVersion":1});
        let mut update = RuntimePreparationUpdate {
            operation_id: "operation".into(),
            stage: PreparationStage::Credentials,
            state: PreparationStepState::Failed,
            source_commit: None,
            error_code: Some("vendor-body-must-not-pass".into()),
            log: None,
        };
        assert_eq!(
            apply(&mut config, &update, 10).unwrap_err().code,
            "invalid_runtime_preparation_error"
        );
        update.error_code = Some("credential_preparation_failed".into());
        assert_eq!(
            apply(&mut config, &update, 11).unwrap(),
            Some("credential_preparation_failed")
        );
        update.stage = PreparationStage::Repository;
        update.state = PreparationStepState::Completed;
        update.error_code = None;
        update.source_commit = Some("a".repeat(40));
        assert_eq!(
            apply(&mut config, &update, 20).unwrap(),
            Some("credential_preparation_failed")
        );
        assert_eq!(
            config["preparation"]["credentials"]["errorCode"],
            "credential_preparation_failed"
        );
        assert_eq!(config["preparation"]["repository"]["state"], "completed");
    }

    #[test]
    fn completed_preparation_without_running_never_fabricates_elapsed_time() {
        let mut config =
            json!({"rustAllocationOperationId":"operation","rustPreparationVersion":1});
        let update = RuntimePreparationUpdate {
            operation_id: "operation".into(),
            stage: PreparationStage::Environment,
            state: PreparationStepState::Completed,
            source_commit: None,
            error_code: None,
            log: None,
        };
        apply(&mut config, &update, 100).unwrap();
        let value = config["startupTimings"]["preparationPhases"]["environment"].clone();
        assert!(value.get("observedDurationMs").is_none());
        assert_eq!(value["lastObservedAtMs"], 100);
        assert!(apply(&mut config, &update, 200).unwrap().is_none());
        assert_eq!(
            config["startupTimings"]["preparationPhases"]["environment"],
            value
        );
    }
    #[test]
    fn preparation_reports_pin_source_and_never_reopen_completed_stages() {
        let mut config =
            json!({"rustAllocationOperationId":"operation","rustPreparationVersion":1});
        let mut u = RuntimePreparationUpdate {
            operation_id: "operation".into(),
            stage: PreparationStage::Repository,
            state: PreparationStepState::Running,
            source_commit: None,
            error_code: None,
            log: None,
        };
        assert_eq!(
            apply(&mut config, &u, 10).unwrap(),
            Some("repository-syncing")
        );
        assert_eq!(apply(&mut config, &u, 11).unwrap(), None);
        u.state = PreparationStepState::Completed;
        u.source_commit = Some("a".repeat(40));
        apply(&mut config, &u, 20).unwrap();
        assert_eq!(config["preparation"]["repository"]["startedAt"], 10);
        assert_eq!(config["resolvedSourceCommit"], "a".repeat(40));
        assert_eq!(
            config["startupTimings"]["preparationPhases"]["repository"]["observedDurationMs"],
            10
        );
        assert_eq!(
            config["startupTimings"]["preparationPhases"]["repository"]["clock"],
            "server-receipt-wall"
        );
        u.source_commit = Some("b".repeat(40));
        assert_eq!(
            apply(&mut config, &u, 21).unwrap_err().code,
            "runtime_repository_source_conflict"
        );
        u.state = PreparationStepState::Running;
        u.source_commit = None;
        assert!(apply(&mut config, &u, 22).unwrap().is_none());
        u.operation_id = "other-operation".into();
        assert_eq!(
            apply(&mut config, &u, 23).unwrap_err().code,
            "runtime_preparation_operation_fenced"
        );
        u.operation_id = "operation".into();
        u.stage = PreparationStage::Ready;
        assert!(apply(&mut config, &u, 24).is_err());
        u.stage = PreparationStage::Environment;
        u.state = PreparationStepState::Failed;
        u.error_code = Some("untrusted vendor text".into());
        assert!(apply(&mut config, &u, 25).is_err());
    }
}
