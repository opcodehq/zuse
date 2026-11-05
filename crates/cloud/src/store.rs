use crate::{access::Access, error::ApiError};
use serde_json::{Value, json};
use sqlx::{
    PgPool, Row,
    postgres::{PgConnectOptions, PgPoolOptions},
};
use std::{str::FromStr, time::Duration};

#[derive(Clone)]
pub struct Store {
    pub(crate) staging_candidate:
        Option<std::sync::Arc<crate::managed_bases::candidate::StagingBasePin>>,
    pub(crate) events: Option<std::sync::Arc<crate::events::hub::Hub>>,
    pub(crate) connections: Option<std::sync::Arc<crate::connections::ConnectionControl>>,
    pub(crate) computers: Option<std::sync::Arc<crate::computers::ComputerControl>>,
    pub(crate) images: Option<std::sync::Arc<crate::images::ImageControl>>,
    pub(crate) github: Option<std::sync::Arc<crate::github::LegacyGithub>>,
    pub(crate) github_joining:
        Option<std::sync::Arc<crate::github::native::organizations::Joining>>,
    pub(crate) github_native: Option<std::sync::Arc<crate::github::native::NativeGithub>>,
    pub(crate) projects: Option<std::sync::Arc<crate::projects::ProjectControl>>,
    pub(crate) project_templates: std::sync::Arc<std::collections::BTreeMap<String, String>>,
    pub(crate) billing: Option<std::sync::Arc<crate::billing::BillingControl>>,
    pub(crate) pool: PgPool,
    pub(crate) transcripts: Option<std::sync::Arc<crate::transcript::Reader>>,
}

/// Private persistence row. Never serialize this object into a response.
pub struct Workspace {
    pub(crate) row: Value,
}

impl Store {
    pub async fn connect_read_only(url: &str) -> anyhow::Result<Self> {
        let options = PgConnectOptions::from_str(url)
            .map_err(|_| anyhow::anyhow!("Invalid cloud PostgreSQL connection configuration"))?
            .application_name("zuse-cloud-api");
        let pool = PgPoolOptions::new()
            .max_connections(8)
            .acquire_timeout(Duration::from_secs(5))
            .after_connect(|connection, _| {
                Box::pin(async move {
                    sqlx::query("SET default_transaction_read_only = on")
                        .execute(&mut *connection)
                        .await?;
                    sqlx::query("SET statement_timeout = '10s'")
                        .execute(&mut *connection)
                        .await?;
                    Ok(())
                })
            })
            .connect_with(options)
            .await
            .map_err(|_| anyhow::anyhow!("Cannot connect to cloud PostgreSQL"))?;
        Ok(Self {
            staging_candidate: None,
            events: None,
            connections: None,
            computers: None,
            pool,
            billing: None,
            images: None,
            projects: None,
            github: None,
            github_native: None,
            github_joining: None,
            project_templates: Default::default(),
            transcripts: None,
        })
    }

    pub fn from_pool(pool: PgPool) -> Self {
        Self {
            staging_candidate: None,
            events: None,
            connections: None,
            computers: None,
            pool,
            billing: None,
            images: None,
            projects: None,
            github: None,
            github_native: None,
            github_joining: None,
            project_templates: Default::default(),
            transcripts: None,
        }
    }

    pub async fn ready(&self) -> Result<(), ApiError> {
        let present: bool = sqlx::query_scalar("SELECT to_regclass('api_cloud_workspaces') IS NOT NULL AND to_regclass('api_entitlements') IS NOT NULL AND to_regclass('api_machines') IS NOT NULL")
            .fetch_one(&self.pool).await?;
        if !present {
            return Err(ApiError::unavailable("cloud_schema_not_ready"));
        }
        if let Some(github) = &self.github_native {
            github.ready().await?;
        }
        if let Some(joining) = &self.github_joining {
            joining.ready().await?;
        }
        if self.images.is_some() {
            let ready:bool=sqlx::query_scalar("SELECT to_regclass('zuse_rust_image_operations') IS NOT NULL AND to_regclass('zuse_rust_control_owners') IS NOT NULL AND to_regclass('api_provider_price_schedule') IS NOT NULL").fetch_one(&self.pool).await?;
            if !ready {
                return Err(ApiError::unavailable("cloud_image_schema_not_ready"));
            }
        }
        if self.transcripts.is_some() {
            let ready: bool = sqlx::query_scalar(
                "SELECT to_regclass('api_cloud_transcript_checkpoints') IS NOT NULL",
            )
            .fetch_one(&self.pool)
            .await?;
            if !ready {
                return Err(ApiError::unavailable("cloud_transcript_schema_not_ready"));
            }
        }
        Ok(())
    }

    /// Launch summaries contain no environment values, credentials or secret bindings.
    pub async fn launch_projects(&self, owner: &str) -> Result<Vec<Value>, ApiError> {
        let present: bool = sqlx::query_scalar("SELECT to_regclass('api_cloud_projects') IS NOT NULL AND to_regclass('api_cloud_project_builds') IS NOT NULL AND to_regclass('api_cloud_snapshots') IS NOT NULL").fetch_one(&self.pool).await?;
        if !present {
            return Ok(vec![]);
        }
        let rows = sqlx::query("SELECT to_jsonb(p) AS data, (SELECT jsonb_build_object('modelConfigurationVersion',b.settings->'rustLaunchModelConfigVersion') FROM api_cloud_project_builds b WHERE b.account_id=p.account_id AND (b.settings->>'rustRuntimeCheckoutProjectId'=p.project_id OR b.settings->'rustRuntimeRepositoryLayouts' ? p.project_id) AND b.provider='boxd' AND b.state='ready' AND b.settings->>'runtimeRpcProtocol'='zuse-rpc-v1' AND b.settings->>'rustRuntimeStorageFormat'='zuse-rust-engine-v1' AND b.settings->>'rustSessionCommandVersion'='1' AND NOT EXISTS(SELECT 1 FROM api_cloud_snapshots s WHERE (s.provider=b.provider OR (b.provider='boxd' AND s.provider='box')) AND s.snapshot_id=b.snapshot_id AND s.state IN ('deleting','deleted')) ORDER BY b.updated_at DESC,b.build_id DESC LIMIT 1) AS runtime FROM api_cloud_projects p WHERE p.account_id=$1 AND p.included=true ORDER BY p.project_id").bind(owner).fetch_all(&self.pool).await?;
        Ok(rows.into_iter().map(|r| {
            let row: Value = r.get("data");
            let runtime: Option<Value> = r.get("runtime");
            json!({"projectId":row["project_id"],"repositoryIdentity":row["repository_identity"],"displayName":row["display_name"].as_str().or(row["repository_url"].as_str()).or(row["project_id"].as_str()),"repositoryUrl":row["repository_url"],"defaultBranch":row["default_branch"].as_str().unwrap_or("main"),"state":row["state"],"prepared":runtime.is_some(),"supportsModelConfiguration":runtime.as_ref().is_some_and(|value| value["modelConfigurationVersion"] == 1),"providerId":"boxd"})
        }).collect())
    }

    pub async fn entitlements(&self, owner: &str) -> Result<Vec<Value>, ApiError> {
        let rows = sqlx::query("SELECT to_jsonb(e) AS data, (SELECT m.machine_id FROM api_machines m WHERE m.account_id=e.account_id AND m.entitlement_id=e.entitlement_id AND m.state <> 'destroyed' ORDER BY m.created_at DESC LIMIT 1) AS machine_id FROM api_entitlements e WHERE account_id=$1 ORDER BY created_at DESC, entitlement_id")
            .bind(owner).fetch_all(&self.pool).await?;
        rows.into_iter().map(|row| {
            let data: Value = row.try_get("data")?;
            let machine: Option<String> = row.try_get("machine_id")?;
            let mut public = json!({"entitlementId":data["entitlement_id"],"kind":data["kind"],"status":data["status"]});
            for (from, to) in [("offer_id", "offerId"), ("paid_through", "paidThrough"), ("credit_balance", "creditBalance")] {
                if !data[from].is_null() { public[to] = data[from].clone(); }
            }
            if let Some(machine) = machine { public["machineId"] = json!(machine); }
            Ok(public)
        }).collect()
    }

    pub async fn workspaces(
        &self,
        access: &Access,
        project: Option<&str>,
    ) -> Result<Vec<Value>, ApiError> {
        let rows: Vec<Value> = sqlx::query_scalar("SELECT to_jsonb(w) FROM api_cloud_workspaces w WHERE account_id=$1 AND ($2::text IS NULL OR project_id=$2) AND (state<>'deleted' OR (to_jsonb(w)->>'deletion_tombstone_expires_at')::bigint>(extract(epoch from clock_timestamp())*1000)::bigint) ORDER BY created_at, workspace_id")
            .bind(&access.owner_id).bind(project).fetch_all(&self.pool).await?;
        let mut visible = Vec::new();
        for row in rows {
            if access
                .workspace_permission(&access.owner_id, &row["request_config"])
                .is_ok()
            {
                visible.push(Workspace { row }.public());
            }
        }
        Ok(visible)
    }

    pub async fn workspace(&self, access: &Access, id: &str) -> Result<Workspace, ApiError> {
        let row: Option<Value> = sqlx::query_scalar("SELECT to_jsonb(w) FROM api_cloud_workspaces w WHERE account_id=$1 AND workspace_id=$2 AND (state<>'deleted' OR (to_jsonb(w)->>'deletion_tombstone_expires_at')::bigint>(extract(epoch from clock_timestamp())*1000)::bigint)")
            .bind(&access.owner_id).bind(id).fetch_optional(&self.pool).await?;
        let row = row.ok_or_else(|| ApiError::not_found("cloud_workspace_not_found"))?;
        access.workspace_permission(&access.owner_id, &row["request_config"])?;
        Ok(Workspace { row })
    }

    /// Read-only inspection: never claims ownership or advances a legacy job.
    pub async fn migration_report(&self) -> Result<Value, ApiError> {
        let read_only: bool =
            sqlx::query_scalar("SELECT current_setting('default_transaction_read_only') = 'on'")
                .fetch_one(&self.pool)
                .await?;
        let counts: Value = sqlx::query_scalar("SELECT jsonb_build_object('workspaces',count(*),'archived',count(*) FILTER (WHERE state='archived'),'deleted',count(*) FILTER (WHERE state='deleted'),'withRetainedMachine',count(*) FILTER (WHERE provider_sandbox_id IS NOT NULL),'withRecoveryArchive',count(*) FILTER (WHERE request_config ? 'recoveryArchive'),'withWrappedTranscriptKey',count(*) FILTER (WHERE wrapped_transcript_key IS NOT NULL),'withActiveLease',count(*) FILTER (WHERE lease_expires_at > (extract(epoch from now())*1000)::bigint)) FROM api_cloud_workspaces")
            .fetch_one(&self.pool).await?;
        let entitlements: i64 = sqlx::query_scalar("SELECT count(*) FROM api_entitlements")
            .fetch_one(&self.pool)
            .await?;
        Ok(
            json!({"mode":"read-only","referenceCommit":crate::REFERENCE_COMMIT,"githubReferenceCommit":crate::github::GITHUB_REFERENCE_COMMIT,
            "databaseReadOnly":read_only,"workspaces":counts,"entitlements":entitlements,"ownershipTransferred":false}),
        )
    }
}

impl Workspace {
    pub fn public(&self) -> Value {
        let row = &self.row;
        let config = &row["request_config"];
        let mut out = json!({});
        // Recovery may name the immutable selected account, never a credential.
        if let Some(agent) = config["agent"]
            .as_str()
            .filter(|agent| matches!(*agent, "codex" | "claude" | "grok"))
        {
            out["agent"] = json!(agent);
            if let Some(connection) = config["agentConnectionId"]
                .as_str()
                .filter(|id| !id.is_empty() && id.len() <= 512 && !id.chars().any(char::is_control))
            {
                out["agentConnectionId"] = json!(connection);
            }
        }
        if crate::runtime::task_commands::command_mode(row) {
            out["commandDeliveryMode"] = json!(zuse_protocol::cloud_task_commands::MODE);
            out["commandAdmissionVersion"] = json!(zuse_protocol::cloud_task_commands::VERSION);
            out["taskCommandCanEdit"] = json!(false);
            out["queueRunSelectionVersion"] =
                json!(zuse_protocol::cloud_task_commands::QUEUE_RUN_SELECTION_VERSION);
        }
        for (from, to) in [
            ("workspace_id", "workspaceId"),
            ("project_id", "projectId"),
            ("build_id", "buildId"),
            ("provider", "providerId"),
            ("branch", "branch"),
            ("base_ref", "baseRef"),
            ("state", "state"),
            ("desired_state", "desiredState"),
            ("status_code", "statusCode"),
            ("runtime_state", "runtimeState"),
            ("revision", "revision"),
            ("chat_id", "chatId"),
            ("initial_session_id", "initialSessionId"),
            ("created_at", "createdAt"),
            ("updated_at", "updatedAt"),
            ("last_activity_at", "lastActivityAt"),
            ("deleted_at", "deletedAt"),
            (
                "deletion_tombstone_expires_at",
                "deletionTombstoneExpiresAt",
            ),
        ] {
            out[to] = row[from].clone();
        }
        if let Some(operation) = config.get("rustDeleteOperation") {
            out["deletion"] = json!({"operationId":operation["id"],"phase":operation["phase"]});
        }
        let ingress = &config["rustPreparationIngress"];
        out["runtimeIngressReady"] = json!(
            ingress["version"] == 1
                && ingress["operationId"] == config["rustAllocationOperationId"]
                && ingress["generation"] == config["runtimeGeneration"]
                && ingress["sourceMachineId"] == row["provider_sandbox_id"]
                && ingress["storageIncarnationId"]
                    == config["rustRuntimeStorage"]["storageIncarnationId"]
                && config["rustStorageEnrollment"]["phase"] == "committed"
                && row["desired_state"] == "ready"
                && matches!(row["state"].as_str(), Some("setup" | "ready"))
        );
        out["allocationRecovery"] = crate::runtime::allocation::rejection::metadata(row);
        let repair = crate::runtime::repair::workspace_metadata(row);
        out["computerRecreation"] =
            crate::runtime::recreation::workspace_metadata(row)["computerRecreation"].clone();
        out["computerRecreationAvailable"] = json!(false);
        out["runtimeRepairAvailable"] = repair["runtimeRepairAvailable"].clone();
        out["runtimeRepair"] = repair["runtimeRepair"].clone();
        out["title"] = json!(row["title"].as_str().or_else(|| config["title"].as_str()));
        if matches!(row["state"].as_str(), Some("archiving" | "archived"))
            || row["desired_state"] == "archived"
        {
            out["archiveStatus"] = json!(crate::archive::status(row));
        }
        out["imageGeneration"] = row["build_id"].clone();
        if config["rustPreparationVersion"] == 1 {
            let mut steps = json!({});
            for stage in ["repository", "environment", "credentials"] {
                let step = &config["preparation"][stage];
                if matches!(
                    step["state"].as_str(),
                    Some("running" | "completed" | "failed")
                ) {
                    let error = match step["errorCode"].as_str() {
                        Some("repository_preparation_failed") => {
                            Some("repository_preparation_failed")
                        }
                        Some("environment_setup_failed") => Some("environment_setup_failed"),
                        Some("credential_preparation_failed") => Some("credential_preparation_failed"),
                        _ => None,
                    };
                    steps[stage] = json!({"state":step["state"],"errorCode":error,"startedAt":step["startedAt"].as_i64(),"updatedAt":step["updatedAt"].as_i64()});
                }
            }
            out["preparation"] = steps;
            if let Some(commit) = config["resolvedSourceCommit"]
                .as_str()
                .filter(|s| matches!(s.len(), 40 | 64) && s.bytes().all(|b| b.is_ascii_hexdigit()))
            {
                out["resolvedSourceCommit"] = json!(commit);
            }
        }
        out["rpcProtocol"] = json!(match config["runtimeRpcProtocol"].as_str() {
            Some(zuse_protocol::cloud_gateway::RUST_RPC) => zuse_protocol::cloud_gateway::RUST_RPC,
            None | Some(zuse_protocol::cloud_gateway::LEGACY_RPC) =>
                zuse_protocol::cloud_gateway::LEGACY_RPC,
            _ => "unsupported",
        });
        for key in ["codexAuthMode", "providerAuthMode"] {
            out[key] = json!(match config[key].as_str() {
                Some("broker-v1") => "broker-v1",
                Some("snapshot-native") => "snapshot-native",
                _ => "legacy-image",
            });
        }
        let mut timings = json!({});
        for key in [
            "requestedAt",
            "forkedAt",
            "resumeRequestedAt",
            "allocatedAt",
            "allocationDurationMs",
            "enrolledAt",
            "runtimeReadyAt",
            "enrollmentDurationMs",
            "networkOpenedAt",
            "credentialInstallDurationMs",
            "repositoryReadyAt",
            "repositoryDurationMs",
            "connectedAt",
            "connectionDurationMs",
            "durableChatCreatedAt",
            "chatCreateDurationMs",
            "agentStartedAt",
            "messageAcceptedAt",
            "agentStartDurationMs",
            "launchDurationMs",
            "providerResumedAt",
            "providerResumeDurationMs",
        ] {
            if config["startupTimings"][key].is_number() {
                timings[key] = config["startupTimings"][key].clone();
            }
        }
        // Startup receipts describe past preparation. They cannot override a
        // current sleep/archive/delete transition or imply a retained agent
        // is already available while its computer is still waking.
        let phase = if matches!(
            row["state"].as_str(),
            Some("paused" | "pausing" | "archived" | "archiving" | "deleting" | "deleted")
        ) {
            match row["state"].as_str().unwrap_or_default() {
                "pausing" => "sleeping",
                state => state,
            }
        } else if row["state"] == "failed" {
            "failed"
        } else if row["status_code"] == "archive-restore-queued"
            || row["status_code"] == "archive-restoring"
        {
            if row["provider_sandbox_id"].is_string() {
                "booting"
            } else {
                "allocating"
            }
        } else if matches!(row["state"].as_str(), Some("queued" | "provisioning"))
            && row["desired_state"] == "ready"
            && config["rustLifecycleOperation"]["action"] == "resume"
            && matches!(
                config["rustLifecycleOperation"]["phase"].as_str(),
                Some("waking" | "launching")
            )
        {
            "resuming"
        } else if row["status_code"] == "agent-running"
            || (row["status_code"] == "ready" && row["runtime_state"] == "online")
            || timings["agentStartedAt"].is_number()
        {
            "running"
        } else if timings["repositoryReadyAt"].is_number() {
            "starting-agent"
        } else if row["runtime_state"] == "online" {
            "syncing-repository"
        } else if row["runtime_state"] == "connecting" {
            "authenticating-runtime"
        } else if row["provider_sandbox_id"].is_string() {
            "booting"
        } else {
            "allocating"
        };
        out["startupTimings"] = timings;
        out["startupPhase"] = json!(phase);
        if let Some(message) = config["startupFailureDiagnostic"]
            .as_str()
            .or_else(|| config["launchErrorCode"].as_str())
        {
            out["failureDiagnostic"] = json!(message);
        }
        out
    }
}

impl Store {
    pub fn with_staging_base_pin(
        mut self,
        pin: Option<std::sync::Arc<crate::managed_bases::candidate::StagingBasePin>>,
    ) -> Self {
        self.staging_candidate = pin;
        self
    }
    pub fn native_github(&self) -> Option<std::sync::Arc<crate::github::native::NativeGithub>> {
        self.github_native.clone()
    }
    pub fn with_native_github_from_env(
        mut self,
        write_pool: Option<PgPool>,
    ) -> anyhow::Result<Self> {
        let enabled =
            std::env::var("ZUSE_CLOUD_GITHUB_WRITE_ACCOUNTS").is_ok_and(|v| !v.trim().is_empty());
        let pool = if enabled {
            write_pool.ok_or_else(|| {
                anyhow::anyhow!("Standalone GitHub requires a writable authority pool")
            })?
        } else {
            self.pool.clone()
        };
        self.github_native = crate::github::native::NativeGithub::from_env(pool)?;
        Ok(self)
    }

    pub fn with_github_organization_authority(
        mut self,
        config: &crate::config::Config,
    ) -> anyhow::Result<Self> {
        if config.organizations_enabled && !config.organization_ids.is_empty() {
            if let Some(native) = &self.github_native {
                if !config
                    .organization_ids
                    .iter()
                    .any(|org| native.enabled(&format!("organization:{org}")))
                {
                    return Ok(self);
                }
                let directory =
                    crate::github::native::organizations::WorkosDirectory::from_config(config)
                        .map_err(|e| anyhow::anyhow!("{}", e.code))?;
                self.github_joining = Some(std::sync::Arc::new(
                    crate::github::native::organizations::Joining::new(
                        native.shared_pool(),
                        std::sync::Arc::new(directory),
                        std::sync::Arc::new(crate::github::native::organizations::GithubRoster(
                            native.clone(),
                        )),
                        config
                            .organization_ids
                            .iter()
                            .filter(|org| native.enabled(&format!("organization:{org}")))
                            .cloned()
                            .collect(),
                    ),
                ));
            }
        }
        Ok(self)
    }
    pub fn github_organization_authority(
        &self,
    ) -> Option<std::sync::Arc<crate::github::native::organizations::Joining>> {
        self.github_joining.clone()
    }

    pub fn with_github_authority(mut self, github: crate::github::LegacyGithub) -> Self {
        self.github = Some(std::sync::Arc::new(github));
        self
    }
}

#[cfg(test)]
mod archive_projection_tests {
    use super::*;

    #[test]
    fn parallel_preparation_projection_exposes_safe_stage_facts_without_secrets() {
        let public = Workspace { row: json!({"request_config":{
            "rustPreparationVersion":1,
            "preparation":{
                "repository":{"state":"running","startedAt":10,"updatedAt":10},
                "credentials":{"state":"failed","errorCode":"credential_preparation_failed",
                    "startedAt":11,"updatedAt":20,"secret":"never-public"},
                "agent":{"state":"completed","secret":"never-public"}
            }
        }}) }.public();
        assert_eq!(public["preparation"]["repository"]["state"], "running");
        assert_eq!(public["preparation"]["credentials"]["errorCode"], "credential_preparation_failed");
        assert!(public["preparation"].get("agent").is_none());
        assert!(!public.to_string().contains("never-public"));
        assert_eq!(public["preparation"]["credentials"]["updatedAt"], 20);
    }

    #[test]
    fn public_recovery_names_only_the_pinned_agent_connection() {
        let public = Workspace { row: json!({"request_config":{"agent":"codex","agentConnectionId":"connection-pinned","agentConnectionOwnerId":"owner-private","credential":"never-public"}}) }.public();
        assert_eq!(public["agent"], "codex");
        assert_eq!(public["agentConnectionId"], "connection-pinned");
        assert!(public.get("agentConnectionOwnerId").is_none());
        assert!(!public.to_string().contains("never-public"));
        for connection in ["", "bad\nconnection"] {
            let public = Workspace {
                row: json!({"request_config":{"agent":"codex","agentConnectionId":connection}}),
            }
            .public();
            assert!(public.get("agentConnectionId").is_none());
        }
    }

    #[test]
    fn lifecycle_status_wins_over_retained_startup_timestamps() {
        for (state, expected) in [
            ("paused", "paused"),
            ("pausing", "sleeping"),
            ("archived", "archived"),
            ("archiving", "archiving"),
            ("deleting", "deleting"),
            ("deleted", "deleted"),
        ] {
            let public=Workspace{row:json!({"state":state,"desired_state":"paused","runtime_state":"offline","status_code":"paused","request_config":{"startupTimings":{"repositoryReadyAt":100,"agentStartedAt":200}}})}.public();
            assert_eq!(
                public["startupPhase"], expected,
                "past agent/repository receipts must not render a sleeping computer as starting or running"
            );
        }
    }
    #[test]
    fn resume_waits_for_current_runtime_instead_of_reusing_old_agent_ready_time() {
        let mut row = json!({"state":"provisioning","desired_state":"ready","runtime_state":"offline","status_code":"resume-waking","request_config":{"startupTimings":{"repositoryReadyAt":100,"agentStartedAt":200},"rustLifecycleOperation":{"action":"resume","phase":"waking"}}});
        assert_eq!(
            Workspace { row: row.clone() }.public()["startupPhase"],
            "resuming"
        );
        row["state"] = json!("ready");
        row["runtime_state"] = json!("online");
        row["status_code"] = json!("ready");
        row["request_config"]["rustLifecycleOperation"]["phase"] = json!("ready");
        assert_eq!(Workspace { row }.public()["startupPhase"], "running");
    }

    #[test]
    fn archive_summary_keeps_recovery_facts_without_private_material() {
        let mut row = json!({"workspace_id":"workspace","chat_id":"chat","initial_session_id":"session",
            "state":"archived","desired_state":"archived","status_code":"archived-retained",
            "provider_sandbox_id":"PRIVATE-MACHINE","archive_requested_at":100,
            "request_config":{"private":"PRIVATE-CONFIG"}});
        let public = Workspace { row: row.clone() }.public();
        assert_eq!(public["archiveStatus"]["machineRetained"], true);
        assert_eq!(public["archiveStatus"]["backupVerified"], false);
        assert_eq!(public["archiveStatus"]["computeRetired"], false);
        assert_eq!(
            public["archiveStatus"]["retentionDeadline"],
            100 + 10 * 86400000_i64
        );
        row["request_config"]["recoveryArchive"] = json!({"version":1,"id":"operation","sourceSandboxId":"PRIVATE-SOURCE",
            "archivedAtMs":100,"expiresAtMs":200,"phase":"retired","verifiedChunks":1,
            "manifest":{"version":1,"operationId":"operation","workspaceId":"workspace","chatId":"chat","sessionId":"session",
                "chunks":["a".repeat(64)],"archiveSha256":"b".repeat(64)}});
        let public = Workspace { row: row.clone() }.public();
        assert_eq!(public["archiveStatus"]["backupVerified"], true);
        assert_eq!(public["archiveStatus"]["computeRetired"], true);
        assert_eq!(public["archiveStatus"]["verifiedChunks"], 1);
        assert!(!public.to_string().contains("PRIVATE"));
        assert!(!public.to_string().contains("manifest"));
        row["request_config"]["recoveryArchive"]["manifest"]["workspaceId"] =
            json!("different-workspace");
        let public = Workspace { row }.public();
        assert_eq!(public["archiveStatus"]["backupVerified"], false);
        assert_eq!(public["archiveStatus"]["computeRetired"], false);
        assert_eq!(
            public["archiveStatus"]["errorCode"],
            "archive-metadata-invalid"
        );
    }

    #[test]
    fn active_workspaces_do_not_receive_an_archive_summary() {
        let public = Workspace {
            row: json!({"state":"running","desired_state":"running"}),
        }
        .public();
        assert!(public.get("archiveStatus").is_none());
    }
}
