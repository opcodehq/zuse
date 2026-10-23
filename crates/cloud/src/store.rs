use crate::{access::Access, error::ApiError};
use serde_json::{Value, json};
use sqlx::{
    PgPool, Row,
    postgres::{PgConnectOptions, PgPoolOptions},
};
use std::{str::FromStr, time::Duration};

#[derive(Clone)]
pub struct Store {
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
            pool,
            billing: None,
            projects: None,
            project_templates: Default::default(),
            transcripts: None,
        })
    }

    pub fn from_pool(pool: PgPool) -> Self {
        Self {
            pool,
            billing: None,
            projects: None,
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
        let rows = sqlx::query("SELECT to_jsonb(p) AS data, EXISTS(SELECT 1 FROM api_cloud_project_builds b WHERE b.account_id=p.account_id AND b.settings->>'rustRuntimeCheckoutProjectId'=p.project_id AND b.provider='boxd' AND b.state='ready' AND b.settings->>'runtimeRpcProtocol'='zuse-rpc-v1' AND b.settings->>'rustRuntimeStorageFormat'='zuse-rust-engine-v1' AND b.settings->>'rustSessionCommandVersion'='1' AND NOT EXISTS(SELECT 1 FROM api_cloud_snapshots s WHERE s.provider=b.provider AND s.snapshot_id=b.snapshot_id AND s.state IN ('deleting','deleted'))) AS prepared FROM api_cloud_projects p WHERE p.account_id=$1 AND p.included=true ORDER BY p.project_id").bind(owner).fetch_all(&self.pool).await?;
        Ok(rows.into_iter().map(|r| {
            let row: Value = r.get("data"); let prepared: bool = r.get("prepared");
            json!({"projectId":row["project_id"],"displayName":row["display_name"].as_str().or(row["repository_url"].as_str()).or(row["project_id"].as_str()),"repositoryUrl":row["repository_url"],"defaultBranch":row["default_branch"].as_str().unwrap_or("main"),"state":row["state"],"prepared":prepared,"providerId":"boxd"})
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
            json!({"mode":"read-only","referenceCommit":crate::REFERENCE_COMMIT,
            "databaseReadOnly":read_only,"workspaces":counts,"entitlements":entitlements,"ownershipTransferred":false}),
        )
    }
}

impl Workspace {
    pub fn public(&self) -> Value {
        let row = &self.row;
        let config = &row["request_config"];
        let mut out = json!({});
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
        out["title"] = json!(row["title"].as_str().or_else(|| config["title"].as_str()));
        out["imageGeneration"] = row["build_id"].clone();
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
        let phase = if row["state"] == "failed" {
            "failed"
        } else if row["status_code"] == "archive-restore-queued"
            || row["status_code"] == "archive-restoring"
        {
            if row["provider_sandbox_id"].is_string() {
                "booting"
            } else {
                "allocating"
            }
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
