//! Authenticated initial creation. Existing identities, billing rows and ciphertext remain authoritative.
use super::*;
use crate::{access::Access, store::Workspace};
use zuse_protocol::cloud::CloudRuntimeMode;

pub use zuse_protocol::cloud::CloudWorkspaceCreateRequest as CreateWorkspace;
fn validate(body: &CreateWorkspace) -> Result<(), ApiError> {
    if body.idempotency_key.is_empty()
        || body.idempotency_key.len() > 512
        || body.project_id.is_empty()
        || body.project_id.len() > 128
        || body.model.is_empty()
        || body.model.len() > 512
        || body
            .first_message
            .as_ref()
            .is_some_and(|s| s.len() > 128 * 1024)
    {
        return Err(ApiError::bad_request("invalid_cloud_workspace_request"));
    }
    if !valid_git_ref(&body.base_ref)
        || body
            .branch
            .as_deref()
            .is_some_and(|s| !valid_git_ref(s) || s == "HEAD")
    {
        return Err(ApiError::bad_request("invalid_git_ref"));
    }
    if body.provider_id != "boxd" {
        return Err(ApiError::bad_request("cloud_provider_unavailable"));
    }
    if body.size_id.as_deref().is_some_and(|s| s != "default") {
        return Err(ApiError::bad_request("cloud_size_unavailable"));
    }
    let agent = if body.agent == "claude" {
        "claude-code"
    } else {
        &body.agent
    };
    let harness = zuse_protocol::HarnessId::try_from(agent.to_owned())
        .map_err(|_| ApiError::bad_request("runtime_agent_unsupported"))?;
    if harness == zuse_protocol::HarnessId::Mock {
        return Err(ApiError::bad_request("runtime_test_harness_disabled"));
    }
    if body.permissions.as_ref().is_some_and(|v| !v.is_empty())
        || matches!(
            body.runtime_mode,
            Some(CloudRuntimeMode::AutoAcceptEdits | CloudRuntimeMode::AutoAcceptEditsAndBash)
        )
    {
        return Err(ApiError::conflict("runtime_permission_import_required"));
    }
    if body.secret_bindings.as_ref().is_some_and(|v| !v.is_empty()) {
        return Err(ApiError::conflict("cloud_secret_runtime_update_required"));
    }
    if body.local_device_id.is_some() {
        return Err(ApiError::conflict("device_bridge_runtime_update_required"));
    }
    if body.fork_source.is_some() {
        return Err(ApiError::bad_request("cloud_fork_endpoint_required"));
    }
    if body
        .initial_message_delivery
        .as_deref()
        .is_some_and(|s| s != "mailbox-v1")
    {
        return Err(ApiError::bad_request("invalid_initial_message_delivery"));
    }
    Ok(())
}

pub fn valid_git_ref(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 255
        && !value.starts_with(['-', '/', '.'])
        && !value.ends_with(['.', '/'])
        && !value.contains("..")
        && !value.contains("//")
        && !value.contains("@{")
        && value
            .split('/')
            .all(|part| !part.starts_with('.') && !part.ends_with(".lock"))
        && value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"._/#-".contains(&c))
}
impl RuntimeControl {
    pub fn with_creation_accounts(mut self, accounts: Vec<String>) -> Self {
        self.creation_accounts = accounts;
        self
    }
    pub fn creation_enabled(&self) -> bool {
        !self.creation_accounts.is_empty()
    }
    pub async fn create_workspace(
        &self,
        access: &Access,
        body: CreateWorkspace,
        now: i64,
    ) -> Result<(Value, bool), ApiError> {
        if access.membership.is_some()
            || access.owner_id != access.actor.account_id
            || !self.creation_accounts.contains(&access.owner_id)
        {
            return Err(ApiError::forbidden("cloud_workspace_creation_disabled"));
        }
        validate(&body)?;
        let owner = &access.owner_id;
        let digest = self.master.digest_api(
            &format!("rust-workspace-create:{owner}"),
            &serde_json::to_string(&body)
                .map_err(|_| ApiError::bad_request("invalid_cloud_workspace_request"))?,
        );
        let mut tx = self.pool.begin().await?;
        // Serializes idempotency and branch admission with other Rust creates for this account.
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
            .bind(format!("cloud-create:{owner}"))
            .execute(&mut *tx)
            .await?;
        let prior:Option<Value>=sqlx::query_scalar("SELECT to_jsonb(w) FROM api_cloud_workspaces w WHERE account_id=$1 AND idempotency_key=$2").bind(owner).bind(&body.idempotency_key).fetch_optional(&mut *tx).await?;
        if let Some(row) = prior {
            if row["request_config"]["rustCreateRequestDigest"] != digest {
                return Err(ApiError::conflict("cloud_workspace_idempotency_conflict"));
            }
            return Ok((create_response(row), false));
        }
        let entitled:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM api_entitlements WHERE account_id=$1 AND kind='cloud-workspace' AND (status IN ('active','grace') OR (status='ended' AND paid_through>$2)) AND (paid_through IS NULL OR paid_through>$2))").bind(owner).bind(now).fetch_one(&mut *tx).await?;
        if !entitled {
            return Err(ApiError::forbidden("cloud_entitlement_required"));
        }
        crate::billing::require_capacity(&mut tx, owner, now).await?;
        let project:Option<Value>=sqlx::query_scalar("SELECT to_jsonb(p) FROM api_cloud_projects p WHERE project_id=$1 AND account_id=$2 AND included=true FOR SHARE").bind(&body.project_id).bind(owner).fetch_optional(&mut *tx).await?;
        let project = project.ok_or_else(|| ApiError::not_found("cloud_project_not_found"))?;
        if project["state"] != "ready" {
            return Err(ApiError::conflict("cloud_project_not_ready"));
        }
        if project["secret_bindings"]
            .as_array()
            .is_some_and(|bindings| !bindings.is_empty())
        {
            return Err(ApiError::conflict("cloud_secret_runtime_update_required"));
        }
        let build:Option<Value>=sqlx::query_scalar("SELECT to_jsonb(b) FROM api_cloud_project_builds b WHERE account_id=$1 AND provider=$2 AND state='ready' AND NOT EXISTS(SELECT 1 FROM api_cloud_snapshots s WHERE s.provider=b.provider AND s.snapshot_id=b.snapshot_id AND s.state IN ('deleting','deleted')) ORDER BY updated_at DESC LIMIT 1 FOR SHARE")
            .bind(owner).bind(&body.provider_id).fetch_optional(&mut *tx).await?;
        let build = build.ok_or_else(|| ApiError::conflict("cloud_project_not_ready"))?;
        // Prepared Rust images attest the checked-out project and guest protocol explicitly.
        // Account-wide cache selection/custom layouts are ported with image preparation.
        if build["settings"]["runtimeRpcProtocol"] != zuse_protocol::cloud_gateway::RUST_RPC
            || build["settings"]["rustRuntimeStorageFormat"]
                != zuse_protocol::cloud::RUST_RUNTIME_STORAGE_FORMAT
            || build["settings"]["rustSessionCommandVersion"] != 1
            || build["settings"]["rustRuntimeCheckoutProjectId"] != body.project_id
            || build["settings"]["providerConnectionId"].is_string()
        {
            return Err(ApiError::conflict("cloud_project_runtime_update_required"));
        }
        let snapshot = build["snapshot_id"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| ApiError::conflict("cloud_project_not_ready"))?;
        let id = format!("workspace_{}", uuid::Uuid::new_v4().simple());
        let chat = format!("chat_{}", uuid::Uuid::new_v4());
        let session = format!("s_{}", uuid::Uuid::new_v4());
        let branch = body
            .branch
            .clone()
            .unwrap_or_else(|| format!("cloud-{}", &id[10..22]));
        let in_use:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM api_cloud_workspaces WHERE project_id=$1 AND branch=$2 AND state<>'deleted')").bind(&body.project_id).bind(&branch).fetch_one(&mut *tx).await?;
        if in_use {
            return Err(ApiError::conflict("cloud_branch_in_use"));
        }
        let command = format!("launch:{id}");
        let turn = format!("turn:{id}");
        let line = body
            .first_message
            .as_deref()
            .unwrap_or(&branch)
            .lines()
            .next()
            .unwrap_or(&branch)
            .trim();
        let title = if line.is_empty() {
            branch.clone()
        } else if line.chars().count() > 80 {
            format!("{}…", line.chars().take(77).collect::<String>())
        } else {
            line.to_owned()
        };
        let mut intent = json!({"commandId":command,"turnId":turn,"title":title,"agent":body.agent,"model":body.model,"permissions":body.permissions.clone().unwrap_or_default(),"runtimeMode":body.runtime_mode.unwrap_or(CloudRuntimeMode::ApprovalRequired)});
        let mailbox = body.initial_message_delivery.as_deref() == Some("mailbox-v1");
        if !mailbox && let Some(message) = body.first_message {
            intent["firstMessage"] = json!(message);
        }
        let ciphertext = self
            .master
            .seal(
                format!("zuse-cloud-workspace-launch-intent-v1\n{owner}\n{id}").as_bytes(),
                intent.to_string().as_bytes(),
            )
            .map_err(|_| ApiError::unavailable("cloud_workspace_launch_intent_unavailable"))?;
        let (_, wrapped) = self
            .master
            .create_transcript_key(owner, &id)
            .map_err(|_| ApiError::unavailable("cloud_transcript_key_unavailable"))?;
        let config = json!({"runtimeGeneration":1,"gatewayEpoch":1,"rustCreateRequestDigest":digest,"rustCommandMailboxEnabled":mailbox,"runtimeRpcProtocol":zuse_protocol::cloud_gateway::RUST_RPC,"title":title,"agent":body.agent,"initialTurnId":turn,"runtimeMode":intent["runtimeMode"],"permissions":intent["permissions"],"repositoryCache":"account-image","rustRepositoryCheckout":{"branch":branch,"baseRef":body.base_ref},"startupTimings":{"requestedAt":now}});
        sqlx::query("INSERT INTO api_cloud_workspaces(workspace_id,account_id,project_id,build_id,provider,runtime_state,chat_id,initial_session_id,branch,base_ref,state,desired_state,status_code,wrapped_transcript_key,idempotency_key,request_config,next_action_at,revision,created_at,updated_at,last_activity_at) VALUES($1,$2,$3,$4,$5,'offline',$6,$7,$8,$9,'queued','ready','provisioning-queued',$10,$11,$12,$13,0,$13,$13,$13)")
            .bind(&id).bind(owner).bind(&body.project_id).bind(build["build_id"].as_str()).bind(&body.provider_id).bind(&chat).bind(&session).bind(&branch).bind(&body.base_ref).bind(wrapped).bind(&body.idempotency_key).bind(config).bind(now).execute(&mut *tx).await?;
        sqlx::query("INSERT INTO api_cloud_workspace_launch_intents(workspace_id,account_id,chat_id,session_id,turn_id,command_id,ciphertext,expires_at,created_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9)").bind(&id).bind(owner).bind(&chat).bind(&session).bind(&turn).bind(&command).bind(ciphertext).bind(now+86_400_000).bind(now).execute(&mut *tx).await?;
        sqlx::query("INSERT INTO zuse_rust_control_owners(resource_kind,resource_id,backend,generation,updated_at) VALUES('workspace',$1,'rust',1,$2)").bind(&id).bind(now).execute(&mut *tx).await?;
        self.queue_allocation_tx(&mut tx, &id, snapshot, now)
            .await?;
        let row: Value = sqlx::query_scalar(
            "SELECT to_jsonb(w) FROM api_cloud_workspaces w WHERE workspace_id=$1",
        )
        .bind(&id)
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok((create_response(row), true))
    }
}
fn create_response(row: Value) -> Value {
    let mailbox = row["request_config"]["rustCommandMailboxEnabled"] == true;
    let mut response = json!({"workspace":Workspace{row}.public()});
    response["chatId"] = response["workspace"]["chatId"].clone();
    response["initialSessionId"] = response["workspace"]["initialSessionId"].clone();
    if mailbox {
        response["initialMessageDelivery"] = json!("mailbox-v1");
    }
    response
}
