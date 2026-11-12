//! Host-authorized control plane access. No access token crosses the viewport RPC boundary.
mod allocation_retry;
pub mod environment;
pub mod events;
pub mod hosted;
pub mod identity;
pub mod launch;
#[cfg(unix)]
pub mod local_terminal;
#[cfg(unix)]
pub mod mirror;
mod recreation;
mod repair;
#[cfg(not(target_arch = "wasm32"))]
mod sidebar_cache;
#[cfg(unix)]
pub mod sync;
pub mod task_commands;
mod unknown_outcome;
#[cfg(test)]
mod workspace_missing_tests;
// The cloud modules share this account authority, independently of workspace login.
use anyhow::{Result, ensure};
use identity::CloudIdentity as Auth;
use serde_json::{Value, json};
use std::{
    sync::{Arc, OnceLock},
    time::Duration,
};
use zuse_model::cloud::{Head, Workspace};

/// Keep HTTP status private until an authoritative route can give it domain meaning.
#[derive(Debug)]
struct CloudHttpFailure(reqwest::StatusCode);
impl std::fmt::Display for CloudHttpFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self.0.as_u16() {
            401 => "Cloud could not verify your session. Reconnect in Settings → General.",
            403 => "You do not have access to this workspace.",
            409 => "The workspace is busy. Wait for its current operation to finish.",
            _ => "Cloud service is unavailable. Try again.",
        })
    }
}
impl std::error::Error for CloudHttpFailure {}

/// The server definitively refused a cloud message or attachment; the text
/// says why. Carried as a type so RPC layers keep it instead of a generic
/// "not confirmed" message.
#[derive(Debug)]
pub(crate) struct TaskRejection(pub &'static str);
impl std::fmt::Display for TaskRejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}
impl std::error::Error for TaskRejection {}
impl TaskRejection {
    pub(crate) fn rpc(&self) -> zuse_rpc::RpcError {
        zuse_rpc::RpcError::Capability(format!(
            "{}{}",
            zuse_protocol::cloud_task_commands::TASK_REJECTED_PREFIX,
            self.0
        ))
    }
}

/// What a rejected cloud message or attachment means for the user.
fn task_rejection(code: &str) -> Option<&'static str> {
    Some(match code {
        "cloud_task_permission_policy_changed" | "cloud_task_permission_policy_invalid" => {
            "This access mode can't be used for this task. Pick another access mode and send again."
        }
        "cloud_task_model_not_installed" | "cloud_task_model_catalog_missing" => {
            "This computer doesn't have that model. Pick another model and send again."
        }
        "cloud_task_model_required" | "cloud_task_agent_required" => {
            "Choose an agent and model for this task, then send again."
        }
        "cloud_task_model_reasoning_invalid" => {
            "That reasoning level isn't available for this model. Pick another and send again."
        }
        "cloud_task_upload_limit" => {
            "Too many attachments are still uploading. Wait for them to finish, then send again."
        }
        "invalid_launch_asset" => "This attachment couldn't be uploaded. Try attaching it again.",
        // The computer refused a delivered message before running it.
        "cloud_command_upload_missing" | "cloud_command_upload_outside_uploads" => {
            "An attached file wasn't on the computer, so this message didn't run. Attach it again and send."
        }
        "cloud_command_assets_too_many" => {
            "Too many attachments for one message, so it didn't run. Remove some and send again."
        }
        "cloud_command_assets_too_large" => {
            "These attachments are too large together, so this message didn't run. Remove some and send again."
        }
        "cloud_command_agent_changed"
        | "cloud_command_model_changed"
        | "cloud_command_agent_not_installed"
        | "cloud_command_agent_catalog_invalid" => {
            "This computer can't run that agent or model, so this message didn't run. Pick another and send again."
        }
        "cloud_command_permission_policy_changed" | "cloud_command_permission_policy_missing" => {
            "This access mode can't be used for this task, so this message didn't run. Pick another access mode and send again."
        }
        "cloud_command_policy_or_runtime_fenced"
        | "cloud_command_checkout_fenced"
        | "cloud_command_admission_update_required"
        | "cloud_command_thread_missing"
        | "cloud_canonical_command_binding_required"
        | "cloud_conversation_scope_rejected"
        | "cloud_interrupt_target_invalid"
        | "cloud_control_runtime_binding_invalid"
        | "command-admission-rejected" => {
            "The computer couldn't accept this message, so it didn't run. Send it again."
        }
        zuse_protocol::cloud_task_commands::RUNTIME_OUTDATED => {
            "This computer is on an older version that can't take files or a different access mode yet. Send without them for now; it will update itself soon."
        }
        _ => return None,
    })
}

pub(crate) fn rpc_error(error: anyhow::Error) -> zuse_rpc::RpcError {
    if let Some(rejection) = error.downcast_ref::<TaskRejection>() {
        return rejection.rpc();
    }
    if let Some(unknown) = error.downcast_ref::<unknown_outcome::UnknownOutcome>() {
        return unknown.rpc();
    }
    match error.downcast::<zuse_rpc::RpcError>() {
        Ok(error) => error,
        Err(error) => zuse_rpc::RpcError::Failed(error.to_string()),
    }
}

pub const DEFAULT_API_URL: &str = crate::account::API_URL;

/// Production cloud origins. Cloud computers stay off against these unless
/// explicitly enabled.
const PRODUCTION_ORIGINS: &[&str] = &["https://api.zuse.sh", "https://api-rust.zuse.sh"];

/// What a request for a cloud computer gets while the beta is off.
pub const BETA_MESSAGE: &str = "Cloud computers are in beta and not available here yet.";

/// Cloud computers are a beta. `ZUSE_CLOUD_BETA=1` turns them on and `=0`
/// off; otherwise they are on for every cloud except production (staging,
/// local development), so production builds never offer them.
pub fn beta_enabled() -> bool {
    // Unit tests exercise cloud code against the default (production) origin.
    if cfg!(test) && std::env::var_os("ZUSE_CLOUD_BETA").is_none() {
        return true;
    }
    beta_enabled_for(
        std::env::var("ZUSE_CLOUD_BETA").ok().as_deref(),
        &crate::api_origins::from_env().cloud,
    )
}

fn beta_enabled_for(flag: Option<&str>, cloud_origin: &str) -> bool {
    match flag.map(|v| v.trim().to_ascii_lowercase()).as_deref() {
        Some("1" | "true" | "on" | "yes") => return true,
        Some("0" | "false" | "off" | "no") => return false,
        _ => {}
    }
    let origin = cloud_origin.trim().trim_end_matches('/');
    !PRODUCTION_ORIGINS.contains(&origin)
}

/// Methods the beta switch covers: cloud computers, their projects, sync and
/// history. The Zuse account and billing stay available everywhere.
pub fn beta_gated(method: &str) -> bool {
    method.starts_with("Cloud")
        && !matches!(
            method,
            zuse_protocol::methods::CLOUD_ACCOUNT_STATUS
                | zuse_protocol::methods::CLOUD_ACCOUNT_SIGN_IN
                | zuse_protocol::methods::CLOUD_ACCOUNT_SIGN_OUT
        )
        && !method.starts_with("CloudBilling")
        && !method.starts_with("CloudPrepaid")
}

const CREATE_LIMIT_REJECTED: &str =
    "This staging account has reached its computer limit. Your new message has not been queued.";
const CREATE_REPOSITORY_REJECTED: &str = "This repository is not available through your connected GitHub App. Choose a connected repository. Your new message has not been queued.";
const CREATE_GITHUB_REJECTED: &str =
    "Connect GitHub before starting this cloud chat. Your new message has not been queued.";

/// Only these explicit admission rejections can release a retained create key.
/// Network failures and generic conflicts remain uncertain and preserve the key.
pub fn definitive_create_rejection(message: &str) -> Option<&'static str> {
    match message {
        CREATE_LIMIT_REJECTED => Some("staging_candidate_computer_limit"),
        CREATE_REPOSITORY_REJECTED => Some("github_repository_access_denied"),
        CREATE_GITHUB_REJECTED => Some("github_runtime_credentials_required"),
        _ => None,
    }
}

pub const LIST: &str = zuse_protocol::methods::CLOUD_WORKSPACES;
pub const READ: &str = zuse_protocol::methods::CLOUD_HISTORY;
#[cfg(not(target_arch = "wasm32"))]
pub fn clear_native_history_cache() -> Result<()> {
    sidebar_cache::clear_all();
    let root = std::env::var_os("ZUSE_CLOUD_HISTORY_CACHE_ROOT")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .map(|home| std::path::PathBuf::from(home).join(".zuse/cache/cloud-history"))
        });
    if let Some(root) = root {
        zuse_cloud_projection::reader::Reader::clear_native_cache(&root)?;
    }
    Ok(())
}
pub const CLEANUP_MIRROR: &str = zuse_protocol::methods::CLOUD_CLEANUP_MIRROR;
pub const DELETE: &str = zuse_protocol::methods::CLOUD_DELETE;
pub const UNARCHIVE: &str = zuse_protocol::methods::CLOUD_UNARCHIVE;
pub fn handles(method: &str) -> bool {
    matches!(
        method,
        LIST | READ
            | zuse_protocol::methods::SLACK_CONNECTIONS
            | zuse_protocol::methods::SLACK_CONNECT
            | zuse_protocol::methods::SLACK_DISCONNECT
            | zuse_protocol::methods::SLACK_CONFIRM
            | zuse_protocol::methods::CLOUD_LOCAL_TERMINAL
            | zuse_protocol::methods::CLOUD_SYNC_STATUS
            | zuse_protocol::methods::CLOUD_SYNC_CONFIGURE
            | zuse_protocol::methods::CLOUD_SYNC_RETRY
            | zuse_protocol::methods::CLOUD_COMPUTER_RECREATION
            | zuse_protocol::methods::CLOUD_ALLOCATION_RETRY
            | zuse_protocol::methods::CLOUD_RUNTIME_REPAIR
            | zuse_protocol::methods::CLOUD_PAUSE
            | zuse_protocol::methods::CLOUD_WORKSPACE
            | zuse_protocol::methods::CLOUD_IMAGE
            | zuse_protocol::methods::CLOUD_IMAGE_DELETE
            | zuse_protocol::methods::CLOUD_IMAGE_BUILD
            | zuse_protocol::methods::CLOUD_PROJECTS
            | zuse_protocol::methods::CLOUD_PROJECT_CONNECT
            | zuse_protocol::methods::CLOUD_PROJECT_REMOVE
            | zuse_protocol::methods::CLOUD_GITHUB
            | zuse_protocol::methods::CLOUD_GITHUB_INSTALL
            | zuse_protocol::methods::CLOUD_CAPABILITIES
            | zuse_protocol::methods::CLOUD_AGENT_CONNECTIONS
            | zuse_protocol::methods::CLOUD_AGENT_CONFIGURE
            | zuse_protocol::methods::CLOUD_AGENT_LOGIN
            | zuse_protocol::methods::CLOUD_AGENT_LOGIN_STATUS
            | zuse_protocol::methods::CLOUD_AGENT_LOGIN_CANCEL
            | zuse_protocol::methods::CLOUD_AGENT_DISCONNECT
            | zuse_protocol::methods::CLOUD_PROJECT_ENVIRONMENT
            | zuse_protocol::methods::CLOUD_PROJECT_ENVIRONMENT_SAVE
            | zuse_protocol::methods::CLOUD_MACHINE
            | zuse_protocol::methods::CLOUD_MACHINE_SAVE
            | zuse_protocol::methods::CLOUD_PROJECT_SECRETS
            | zuse_protocol::methods::CLOUD_PROJECT_SECRET_SAVE
            | zuse_protocol::methods::CLOUD_PROJECT_SECRET_REVOKE
            | zuse_protocol::methods::CLOUD_CONTEXT
            | zuse_protocol::methods::CLOUD_EVENTS
            | zuse_protocol::methods::CLOUD_CREATE
            | zuse_protocol::methods::CLOUD_TASK_COMMAND
            | zuse_protocol::methods::CLOUD_TASK_ASSET
            | zuse_protocol::methods::CLOUD_BILLING_SUMMARY
            | zuse_protocol::methods::CLOUD_BILLING_USAGE
            | zuse_protocol::methods::CLOUD_BILLING_CAP
            | zuse_protocol::methods::CLOUD_PREPAID_BALANCE
            | zuse_protocol::methods::CLOUD_PREPAID_CHECKOUT
            | zuse_protocol::methods::CLOUD_BILLING_ENTITLEMENTS
            | zuse_protocol::methods::CLOUD_BILLING_CHECKOUT
            | zuse_protocol::methods::CLOUD_BILLING_PORTAL
            | UNARCHIVE
            | DELETE
            | CLEANUP_MIRROR
            | zuse_protocol::methods::CLOUD_ENVIRONMENT
            | zuse_protocol::methods::CLOUD_RPC
    )
}
pub struct CloudApi {
    account_events: events::Manager,
    task_commands: task_commands::Manager,
    origin: reqwest::Url,
    http: reqwest::Client,
    history: zuse_cloud_projection::reader::Reader,
    loopback: bool,
    hosted: bool,
    environments: environment::Manager,
    #[cfg(unix)]
    mirrors: mirror::Manager,
    #[cfg(unix)]
    syncs: sync::Manager,
    #[cfg(unix)]
    local_terminals: Arc<local_terminal::Manager>,
}
impl CloudApi {
    pub fn new(origin: &str) -> Result<Self> {
        let history = zuse_cloud_projection::reader::Reader::new(origin)?;
        let origin = reqwest::Url::parse(origin)?;
        let loopback = origin.host_str().is_some_and(|h| {
            h == "localhost"
                || h.parse::<std::net::IpAddr>()
                    .is_ok_and(|ip| ip.is_loopback())
        });
        Ok(Self {
            account_events: events::Manager::default(),
            task_commands: task_commands::Manager::default(),
            origin,
            http: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(Duration::from_secs(20))
                .build()?,
            history,
            loopback,
            hosted: false,
            environments: environment::Manager::default(),
            #[cfg(unix)]
            syncs: sync::Manager::default(),
            #[cfg(unix)]
            local_terminals: Arc::default(),
            #[cfg(unix)]
            mirrors: mirror::Manager::new(
                std::env::var_os("ZUSE_CLOUD_MIRROR_ROOT")
                    .map(std::path::PathBuf::from)
                    .unwrap_or_else(|| {
                        std::env::var_os("HOME")
                            .map(std::path::PathBuf::from)
                            .unwrap_or_else(std::env::temp_dir)
                            .join(".zuse/cloud")
                    }),
            ),
        })
    }
    pub(crate) fn hosted(origin: &str) -> Result<Self> {
        let mut api = Self::new(origin)?;
        api.hosted = true;
        api.task_commands = task_commands::Manager::hosted();
        Ok(api)
    }
    fn shared() -> Result<Arc<Self>> {
        static API: OnceLock<Result<Arc<CloudApi>, String>> = OnceLock::new();
        API.get_or_init(|| {
            Self::new(&crate::api_origins::from_env().cloud)
                .map(Arc::new)
                .map_err(|_| "Cloud service configuration is invalid.".into())
        })
        .clone()
        .map_err(anyhow::Error::msg)
    }
    fn local_cleanup(&self, account: &str, row: &mut Workspace, retry: bool) {
        if self.hosted {
            return;
        }
        let admitted = matches!(row.state.as_str(), "archived" | "deleting" | "deleted");
        #[cfg(unix)]
        {
            self.local_terminals.observe(account, row);
            row.local_mirror_cleanup =
                self.mirrors
                    .observe(account, &row.workspace_id, admitted, retry);
            self.syncs.observe(account, row);
        }
        #[cfg(not(unix))]
        {
            let _ = (account, row, retry, admitted);
        }
    }
    async fn request(&self, token: &str, path: &[&str], body: Option<Value>) -> Result<Value> {
        self.request_scoped(token, path, body, None, &[]).await
    }
    async fn request_scoped(
        &self,
        token: &str,
        path: &[&str],
        body: Option<Value>,
        organization: Option<&str>,
        query: &[(String, String)],
    ) -> Result<Value> {
        let verb = if body.is_some() {
            reqwest::Method::POST
        } else {
            reqwest::Method::GET
        };
        self.request_scoped_verb(token, path, body, organization, query, verb)
            .await
    }
    async fn request_scoped_verb(
        &self,
        token: &str,
        path: &[&str],
        body: Option<Value>,
        organization: Option<&str>,
        query: &[(String, String)],
        verb: reqwest::Method,
    ) -> Result<Value> {
        let creating_workspace =
            verb == reqwest::Method::POST && path.len() == 3 && path[2] == "workspaces";
        let mut url = self.origin.clone();
        url.path_segments_mut()
            .map_err(|_| anyhow::anyhow!("Cloud service configuration is invalid."))?
            .clear()
            .extend(path.iter().copied());
        let request = self.http.request(verb, url);
        let request = if let Some(body) = body {
            request.json(&body)
        } else {
            request
        };
        let request = request.query(query);
        let request = if let Some(org) = organization {
            request.header("x-zuse-workspace", format!("organization:{org}"))
        } else {
            request
        };
        let mut response = request
            .bearer_auth(token)
            .send()
            .await
            .map_err(|_| anyhow::anyhow!("Cloud service is unavailable. Try again."))?;
        let status = response.status();
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| anyhow::anyhow!("Cloud service is unavailable."))?
        {
            ensure!(
                bytes.len() + chunk.len() <= 8 * 1024 * 1024,
                "Cloud response is too large."
            );
            bytes.extend_from_slice(&chunk);
        }
        if !status.is_success() {
            #[cfg(not(target_arch = "wasm32"))]
            if !self.hosted
                && matches!(status.as_u16(), 401 | 403)
                && (path == ["v1", "cloud", "projects"] || path == ["v1", "cloud", "workspaces"])
            {
                sidebar_cache::clear_all();
                // Keep authority failures distinct from repository editing errors.
                return Err(CloudHttpFailure(status).into());
            }
            #[cfg(not(target_arch = "wasm32"))]
            if !self.hosted
                && matches!(status.as_u16(), 401 | 403)
                && path.starts_with(&["v1", "cloud", "workspaces"])
                && path.len() >= 4
            {
                let _ = clear_native_history_cache();
            }
            if path.last() == Some(&"tool-output") {
                let body: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
                if body["error"] == "tool_output_not_published" {
                    anyhow::bail!("This output has not finished syncing. Retry shortly.");
                }
            }
            if path.get(2) == Some(&"agents") {
                let error: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
                anyhow::bail!(
                    "{}",
                    match error["error"].as_str() {
                        Some("cloud_agent_connection_writes_disabled") =>
                            "Agent connections are not enabled on this service yet.",
                        Some("invalid_agent_connection" | "invalid_device_login") =>
                            "Check the credential and sign-in method for this agent.",
                        Some("agent_connection_idempotency_conflict") =>
                            "This connection request already has a different result. Refresh connections before retrying.",
                        Some("agent_login_not_found" | "agent_connection_not_found") =>
                            "This connection is no longer available. Refresh and try again.",
                        Some("workspace_access_denied") =>
                            "An administrator role is required to manage shared agent connections.",
                        _ =>
                            "Agent connections are unavailable. Refresh to check the latest status.",
                    }
                );
            }
            if path.get(2) == Some(&"image") {
                let error: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
                if error["error"] == "cloud_image_writes_disabled" {
                    anyhow::bail!(
                        "Automatic environment preparation is not enabled on this service yet."
                    );
                }
            }
            if path.get(2) == Some(&"slack") {
                let error: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
                anyhow::bail!(
                    "{}",
                    match error["error"].as_str() {
                        Some("slack_project_not_ready") =>
                            "Prepare or rebuild this repository's cloud image before connecting Slack.",
                        Some("slack_organization_runtime_unavailable") =>
                            "Organization cloud runtimes are not available yet. Choose Personal for Slack tasks.",
                        Some("slack_model_unavailable") =>
                            "Choose an available cloud model for Slack.",
                        Some("slack_workspace_mismatch") =>
                            "Your workspace changed. Refresh Slack settings.",
                        _ if status.as_u16() == 404 =>
                            "Slack is not enabled on this cloud service.",
                        _ => "Slack settings are unavailable. Refresh and try again.",
                    }
                );
            }
            if path
                .get(2)
                .is_some_and(|p| matches!(*p, "launch-assets" | "workspaces"))
            {
                let error: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
                let creating = creating_workspace
                    && status.is_client_error()
                    && !matches!(status.as_u16(), 408 | 429);
                let actionable = match error["error"].as_str() {
                    Some("staging_candidate_computer_limit") if creating => {
                        Some(CREATE_LIMIT_REJECTED)
                    }
                    Some(
                        "github_repository_access_denied"
                        | "github_installation_required"
                        | "github_repository_not_selected",
                    ) if creating => Some(CREATE_REPOSITORY_REJECTED),
                    Some(
                        "github_runtime_credentials_required"
                        | "github_user_connection_required"
                        | "github_user_reconnect_required"
                        | "github_reconnect_required",
                    ) if creating => Some(CREATE_GITHUB_REJECTED),
                    Some("cloud_project_not_ready" | "managed_base_unavailable") => Some(
                        "This service has no compatible computer environment for the repository yet. Your draft is preserved.",
                    ),
                    Some("agent_connection_required") => {
                        Some("Choose a connected coding-agent account before starting this task.")
                    }
                    Some("environment_revision_not_found") => Some(
                        "The selected environment version is unavailable. Refresh Environment settings before starting.",
                    ),
                    Some("environment_runtime_update_required") => Some(
                        "This service needs a compatible runtime for environment setup. Your draft is preserved.",
                    ),
                    Some("cloud_launch_attachments_runtime_update_required") => Some(
                        "Cloud attachments need a runtime update on this service. Your draft is preserved.",
                    ),
                    Some("cloud_launch_upload_expired") => Some(
                        "These cloud uploads expired. Start a new cloud draft and attach the files again.",
                    ),
                    Some("launch_asset_incomplete") => {
                        Some("The cloud attachment upload is incomplete. Retry the same message.")
                    }
                    Some(
                        "launch_asset_integrity_failed"
                        | "launch_asset_manifest_conflict"
                        | "launch_asset_chunk_conflict",
                    ) => Some(
                        "The cloud attachment could not be verified. Keep the original file and start a new draft.",
                    ),
                    Some("cloud_launch_asset_schema_not_ready") => {
                        Some("Cloud launch attachments are not enabled on this service yet.")
                    }
                    _ => None,
                };
                if let Some(message) = actionable {
                    anyhow::bail!("{message}");
                }
            }
            if path.starts_with(&["v1", "cloud", "billing"]) || path.starts_with(&["v1", "billing"])
            {
                let value: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
                let message = match value["error"].as_str() {
                    Some("cloud_billing_subscription_required") => {
                        "An active cloud billing period is required."
                    }
                    Some("billing_cap_below_incurred") => {
                        "The cap cannot be lower than overage already incurred."
                    }
                    Some("billing_cap_idempotency_conflict") => {
                        "This cap request was already used with a different amount. Refresh and retry."
                    }
                    Some("cloud_billing_owner_not_rust" | "cloud_billing_writes_disabled") => {
                        "Cloud billing changes are still managed by the original service."
                    }
                    Some("machine_subscription_exists") => {
                        "This account already has a subscription. Refresh Cloud or open Manage billing."
                    }
                    Some("billing_approval_pending") => {
                        "Checkout is not enabled for this service yet."
                    }
                    Some("billing_customer_reconciliation_required") => {
                        "Stripe customer recovery is still in progress. Retry checkout to continue the saved recovery."
                    }
                    Some("billing_subscription_mismatch") => {
                        "The existing subscription could not be verified. Open Manage billing or contact support."
                    }
                    Some("invalid_prepaid_amount") => {
                        "Choose $10, $25, $50 or $100 of prepaid credit."
                    }
                    Some("billing_credit_reconciliation_required") => {
                        "This credit purchase needs payment reconciliation. Refresh the balance and contact support before purchasing again."
                    }
                    Some("workspace_access_denied") => {
                        "You need an administrator or billing role to manage this account's billing."
                    }
                    Some("billing_provider_ambiguous") => {
                        "This account has subscriptions with multiple billing providers. Contact support to select the correct portal."
                    }
                    Some("billing_provider_unavailable") => {
                        "The billing provider is unavailable. Try opening billing again."
                    }
                    Some("billing_operation_incomplete") => {
                        "Payment processing is still pending or needs recovery. Retry to reopen the saved checkout; do not create a second purchase."
                    }
                    Some(
                        "billing_enrollment_incomplete"
                        | "billing_enrollment_required"
                        | "billing_adoption_required",
                    ) => {
                        "Billing setup needs to finish for this account. Contact support before purchasing again."
                    }
                    Some("billing_compute_catalog_mismatch") => {
                        "Cloud checkout pricing is not configured correctly. Contact support."
                    }
                    Some("model_cost_reconciliation_required") => {
                        "This model request needs billing reconciliation. Its recorded payment will not be charged again."
                    }
                    Some("shared_credit_exhausted") => {
                        "Shared credits are exhausted. Add credits in workspace billing to continue compute and models."
                    }
                    Some(
                        "billing_shared_allowance_required"
                        | "billing_shared_cutover_reconciliation_required",
                    ) => {
                        "Shared billing needs configuration or balance reconciliation. Contact support."
                    }
                    Some("invalid_billing_cap") => "Enter a valid nonnegative monthly cap.",
                    _ => "Cloud billing is unavailable. Refresh and try again.",
                };
                anyhow::bail!("{message}");
            }
            if path
                .get(2)
                .is_some_and(|s| *s == "projects" || *s == "github")
            {
                let error = serde_json::from_slice::<Value>(&bytes)
                    .ok()
                    .and_then(|v| v["error"].as_str().map(str::to_owned));
                anyhow::bail!(
                    "{}",
                    match error.as_deref() {
                        Some("invalid_repository") =>
                            "Check the repository URL, branch and environment settings.",
                        Some(
                            "invalid_environment_recipe"
                            | "invalid_environment_command"
                            | "invalid_startup_service",
                        ) =>
                            "Check the setup commands, unique service names and readiness settings.",
                        Some("environment_secret_binding_required") =>
                            "Store credentials in a secret binding, not in ordinary environment values.",
                        Some("environment_revision_changed") =>
                            "Environment settings changed elsewhere. Reload the saved version before saving.",
                        Some("environment_managed_in_repository") =>
                            "This environment is managed by .zuse/setup.json. Edit that file in the repository.",
                        Some("cloud_environment_writes_disabled") =>
                            "Environment settings are not enabled for this account yet.",
                        Some("cloud_entitlement_required") =>
                            "A Cloud subscription is required to allocate compute.",
                        Some("cloud_project_secret_vault_required") =>
                            "Use the cloud secret vault for repository secrets.",
                        Some("cloud_project_writes_disabled" | "cloud_project_owner_not_rust") =>
                            "Repository management is not enabled for this account yet.",
                        Some("cloud_github_unavailable") =>
                            "GitHub is unavailable. Refresh and try again.",
                        Some("workspace_access_denied") =>
                            "An administrator role is required to manage repositories.",
                        Some("cloud_project_not_found") =>
                            "This repository is no longer included. Refresh the list.",
                        _ => "Repository management is unavailable. Refresh and try again.",
                    }
                );
            }
            if path == ["v1", "cloud", "context"] && status == reqwest::StatusCode::CONFLICT {
                let value: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
                let message = match value["error"].as_str() {
                    Some("staging_managed_base_candidate_expired") => {
                        "Cloud computer setup expired on the server. Your projects and chats are still available."
                    }
                    _ => {
                        "Cloud setup is temporarily unavailable. Refresh Cloud settings to try again."
                    }
                };
                anyhow::bail!("{message}");
            }
            // Message and attachment rejections name their reason; "busy"
            // would send the user waiting for something that never ends.
            if status.is_client_error()
                && path.first() == Some(&"v1")
                && path
                    .iter()
                    .any(|part| *part == "task-commands" || *part == "assets")
            {
                let value: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
                if let Some(unknown) = value["error"]
                    .as_str()
                    .and_then(unknown_outcome::UnknownOutcome::from_code)
                {
                    return Err(unknown.into());
                }
                if let Some(message) = value["error"].as_str().and_then(task_rejection) {
                    return Err(TaskRejection(message).into());
                }
            }
            return Err(CloudHttpFailure(status).into());
        }
        Ok(serde_json::from_slice(&bytes)?)
    }
    pub async fn handle(&self, auth: &Auth, method: &str, params: Value) -> Result<Value> {
        // A dev/local profile has no production cloud identity. Loopback is solely useful for local acceptance.
        if !auth.workos_enabled() && !self.loopback {
            return if method == LIST {
                Ok(json!({"accountId":null,"workspaces":[]}))
            } else {
                anyhow::bail!("Sign in to access cloud workspaces.")
            };
        }
        let Some(account) = auth.user_id() else {
            return if method == LIST {
                Ok(json!({"accountId":null,"workspaces":[]}))
            } else {
                anyhow::bail!("Sign in to access cloud workspaces.")
            };
        };
        let organization = auth.state().org_id().map(str::to_owned);
        if let Some(expected) = params.get("expectedAccountId") {
            ensure!(
                expected.as_str() == Some(account.as_str())
                    && params["expectedOrganizationId"].as_str() == auth.state().org_id(),
                "Your account changed. Reopen this view."
            );
        }
        // Catalog reads are provisional paint only; never request a token or a guest.
        if params["cacheOnly"] == true
            && matches!(method, zuse_protocol::methods::CLOUD_PROJECTS | LIST)
        {
            #[cfg(not(target_arch = "wasm32"))]
            if !self.hosted {
                return Ok(sidebar_cache::read(
                    self.origin.as_str(),
                    &account,
                    organization.as_deref(),
                    method,
                ));
            }
            return Ok(
                json!({"accountId":account,"organizationId":organization,"apiOrigin":self.origin.as_str(),"provisional":true,"cacheMiss":true}),
            );
        }
        // This branch only decrypts a platform-protected local snapshot. It does
        // not refresh authorization or create an execution connection. The UI
        // always sends a distinct fresh read to validate access and current data.
        if method == READ && params["cacheOnly"] == true {
            if self.hosted {
                return Ok(
                    json!({"accountId":account,"organizationId":organization,"head":null,"provisional":true}),
                );
            }
            #[cfg(not(target_arch = "wasm32"))]
            {
                let root = std::env::var_os("ZUSE_CLOUD_HISTORY_CACHE_ROOT")
                    .map(std::path::PathBuf::from)
                    .or_else(|| {
                        std::env::var_os("HOME").map(|home| {
                            std::path::PathBuf::from(home).join(".zuse/cache/cloud-history")
                        })
                    });
                let scope = organization
                    .as_ref()
                    .map(|org| format!("organization:{org}"))
                    .unwrap_or_else(|| "personal".into());
                let history = self.history.clone().with_scope(&scope)?;
                let head = root
                    .and_then(|root| history.with_page_cache(&root, &account).ok())
                    .and_then(|reader| {
                        reader
                            .provisional_head(
                                params["workspaceId"].as_str().unwrap_or_default(),
                                params["sessionId"].as_str(),
                            )
                            .ok()
                            .flatten()
                    });
                return Ok(
                    json!({"accountId":account,"organizationId":organization,"head":head,"provisional":true}),
                );
            }
            #[cfg(target_arch = "wasm32")]
            return Ok(json!({"head":null,"provisional":true}));
        }
        let token = auth
            .access_token()
            .await
            .map_err(|_| anyhow::anyhow!("Could not refresh your Cloud session. Check your connection or reconnect in Settings → General."))?;
        ensure!(
            auth.user_id().as_deref() == Some(account.as_str())
                && auth.state().org_id() == organization.as_deref(),
            "Your account changed. Reopen this view."
        );
        #[cfg(unix)]
        if !self.hosted
            && organization.is_none()
            && matches!(
                method,
                LIST | READ
                    | DELETE
                    | UNARCHIVE
                    | CLEANUP_MIRROR
                    | zuse_protocol::methods::CLOUD_WORKSPACE
            )
        {
            self.syncs.hydrate(self, auth, &account)?;
        }
        let mut result = match method {
            zuse_protocol::methods::CLOUD_TASK_COMMAND => {
                ensure!(
                    params["expectedAccountId"].as_str() == Some(account.as_str())
                        && params["expectedOrganizationId"].as_str() == organization.as_deref(),
                    "The cloud account changed. Reopen this computer."
                );
                let workspace = params["workspaceId"]
                    .as_str()
                    .filter(|v| !v.is_empty() && v.len() <= 512)
                    .ok_or_else(|| anyhow::anyhow!("Select a cloud computer."))?;
                let request: zuse_protocol::cloud_task_commands::Request =
                    serde_json::from_value(params["request"].clone())?;
                let body = Some(serde_json::to_value(&request)?);
                let response = self
                    .request_scoped(
                        &token,
                        &["v1", "cloud", "workspaces", workspace, "task-commands"],
                        body.clone(),
                        organization.as_deref(),
                        &[],
                    )
                    .await;
                let response = match response {
                    Ok(value) => value,
                    Err(_) => {
                        self.request_scoped(
                            &token,
                            &["v1", "cloud", "workspaces", workspace, "task-commands"],
                            body,
                            organization.as_deref(),
                            &[],
                        )
                        .await?
                    }
                };
                ensure!(
                    response["durablyAdmitted"] == true
                        && response["commandId"] == request.command_id,
                    "The cloud action did not return its durable admission identity. Check the original action before retrying."
                );
                let mut response = response;
                if let Some(attachment) = task_commands::passive_attachment(
                    auth,
                    &account,
                    organization.as_deref(),
                    workspace,
                    &request,
                    &response,
                )? {
                    response["passiveAttachment"] = attachment;
                }
                json!({"accountId":account,"data":response})
            }
            zuse_protocol::methods::CLOUD_TASK_ASSET => {
                ensure!(
                    params["expectedAccountId"].as_str() == Some(account.as_str())
                        && params["expectedOrganizationId"].as_str() == organization.as_deref(),
                    "The cloud account changed. Reopen this computer."
                );
                let workspace = params["workspaceId"]
                    .as_str()
                    .filter(|v| !v.is_empty() && v.len() <= 512)
                    .ok_or_else(|| anyhow::anyhow!("Select a cloud computer."))?;
                let name = params["method"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("Missing cloud asset action."))?;
                let value = task_commands::assets::forward(
                    self,
                    auth,
                    &account,
                    organization.as_deref(),
                    workspace,
                    name,
                    params["params"].clone(),
                )
                .await?;
                json!({"accountId":account,"data":value})
            }
            zuse_protocol::methods::SLACK_CONNECTIONS
            | zuse_protocol::methods::SLACK_CONNECT
            | zuse_protocol::methods::SLACK_DISCONNECT
            | zuse_protocol::methods::SLACK_CONFIRM => {
                let mut body = params.clone();
                if let Some(map) = body.as_object_mut() {
                    map.remove("expectedAccountId");
                    map.remove("expectedOrganizationId");
                }
                let (path, body) = match method {
                    zuse_protocol::methods::SLACK_CONNECT => (
                        vec!["v1", "cloud", "slack", "connect"],
                        Some(serde_json::to_value(serde_json::from_value::<
                            zuse_protocol::slack::ConnectRequest,
                        >(body)?)?),
                    ),
                    zuse_protocol::methods::SLACK_DISCONNECT => {
                        if let Some(map) = body.as_object_mut() {
                            map.remove("workspaceId");
                        }
                        (vec!["v1", "cloud", "slack", "disconnect"], Some(body))
                    }
                    zuse_protocol::methods::SLACK_CONFIRM => {
                        if let Some(map) = body.as_object_mut() {
                            map.remove("workspaceId");
                        }
                        (vec!["v1", "cloud", "slack", "confirm"], Some(body))
                    }
                    _ => (vec!["v1", "cloud", "slack"], None),
                };
                let selected = match params["workspaceId"].as_str() {
                    None => organization.clone(),
                    Some("personal") => None,
                    Some(value) => Some(
                        value
                            .strip_prefix("organization:")
                            .filter(|id| {
                                !id.is_empty()
                                    && id.len() <= 128
                                    && id.bytes().all(|b| {
                                        b.is_ascii_alphanumeric() || b == b'_' || b == b'-'
                                    })
                            })
                            .ok_or_else(|| anyhow::anyhow!("Invalid Slack workspace selection."))?
                            .to_owned(),
                    ),
                };
                let value = self
                    .request_scoped(&token, &path, body, selected.as_deref(), &[])
                    .await?;
                if method == zuse_protocol::methods::SLACK_CONNECT {
                    let url = reqwest::Url::parse(value["url"].as_str().ok_or_else(|| {
                        anyhow::anyhow!("Slack returned an invalid connection link.")
                    })?)?;
                    ensure!(
                        url.origin() == self.origin.origin()
                            && url.path() == "/slack/oauth/start"
                            && url.username().is_empty()
                            && url.password().is_none()
                            && url.fragment().is_none(),
                        "Slack returned an invalid connection link."
                    );
                }
                json!({"accountId":account,"organizationId":organization,"data":value})
            }

            zuse_protocol::methods::CLOUD_PREPAID_BALANCE
            | zuse_protocol::methods::CLOUD_PREPAID_CHECKOUT => {
                let org = organization.clone();
                let (path, body) = if method == zuse_protocol::methods::CLOUD_PREPAID_CHECKOUT {
                    let request: zuse_protocol::cloud_billing::PrepaidCheckoutRequest =
                        serde_json::from_value(params)?;
                    (
                        vec!["v1", "billing", "prepaid", "checkout"],
                        Some(serde_json::to_value(request)?),
                    )
                } else {
                    (vec!["v1", "billing", "prepaid"], None)
                };
                let value = self
                    .request_scoped(&token, &path, body, org.as_deref(), &[])
                    .await?;
                json!({"accountId":account,"organizationId":org,"data":value})
            }
            zuse_protocol::methods::CLOUD_BILLING_ENTITLEMENTS
            | zuse_protocol::methods::CLOUD_BILLING_CHECKOUT => {
                let org = organization.clone();
                let (operation, body) = if method == zuse_protocol::methods::CLOUD_BILLING_CHECKOUT
                {
                    let request: zuse_protocol::cloud_billing::BillingCheckoutRequest =
                        serde_json::from_value(params)?;
                    ("checkout", Some(serde_json::to_value(request)?))
                } else {
                    ("entitlements", None)
                };
                let value = self
                    .request_scoped(
                        &token,
                        &["v1", "billing", operation],
                        body,
                        org.as_deref(),
                        &[],
                    )
                    .await?;
                json!({"accountId":account,"organizationId":org,"data":value})
            }
            zuse_protocol::methods::CLOUD_BILLING_PORTAL => {
                let org = organization.clone();
                let value = self
                    .request_scoped(
                        &token,
                        &["v1", "billing", "portal"],
                        Some(json!({})),
                        org.as_deref(),
                        &[],
                    )
                    .await?;
                json!({"accountId":account,"organizationId":org,"data":value})
            }
            zuse_protocol::methods::CLOUD_BILLING_SUMMARY
            | zuse_protocol::methods::CLOUD_BILLING_USAGE
            | zuse_protocol::methods::CLOUD_BILLING_CAP => {
                let mut query = Vec::new();
                let mut body = None;
                let operation = match method {
                    zuse_protocol::methods::CLOUD_BILLING_USAGE => {
                        let request: zuse_protocol::cloud_billing::BillingUsageRequest =
                            serde_json::from_value(params)?;
                        if let Some(cursor) = request.cursor {
                            query.push(("cursor".into(), cursor));
                        }
                        query.push((
                            "limit".into(),
                            request.limit.unwrap_or(50).clamp(1, 100).to_string(),
                        ));
                        "usage"
                    }
                    zuse_protocol::methods::CLOUD_BILLING_CAP => {
                        let request: zuse_protocol::cloud_billing::BillingCapRequest =
                            serde_json::from_value(params)?;
                        body = Some(serde_json::to_value(request)?);
                        "cap"
                    }
                    _ => "summary",
                };
                let org = organization.clone();
                let value = self
                    .request_scoped(
                        &token,
                        &["v1", "cloud", "billing", operation],
                        body,
                        org.as_deref(),
                        &query,
                    )
                    .await?;
                json!({"accountId":account,"organizationId":org,"data":value})
            }
            zuse_protocol::methods::CLOUD_IMAGE
            | zuse_protocol::methods::CLOUD_IMAGE_DELETE
            | zuse_protocol::methods::CLOUD_IMAGE_BUILD
            | zuse_protocol::methods::CLOUD_PROJECTS
            | zuse_protocol::methods::CLOUD_PROJECT_CONNECT
            | zuse_protocol::methods::CLOUD_PROJECT_REMOVE
            | zuse_protocol::methods::CLOUD_GITHUB
            | zuse_protocol::methods::CLOUD_GITHUB_INSTALL
            | zuse_protocol::methods::CLOUD_CAPABILITIES
            | zuse_protocol::methods::CLOUD_AGENT_CONNECTIONS
            | zuse_protocol::methods::CLOUD_AGENT_CONFIGURE
            | zuse_protocol::methods::CLOUD_AGENT_LOGIN
            | zuse_protocol::methods::CLOUD_AGENT_LOGIN_STATUS
            | zuse_protocol::methods::CLOUD_AGENT_LOGIN_CANCEL
            | zuse_protocol::methods::CLOUD_AGENT_DISCONNECT
            | zuse_protocol::methods::CLOUD_PROJECT_ENVIRONMENT
            | zuse_protocol::methods::CLOUD_PROJECT_ENVIRONMENT_SAVE
            | zuse_protocol::methods::CLOUD_MACHINE
            | zuse_protocol::methods::CLOUD_MACHINE_SAVE
            | zuse_protocol::methods::CLOUD_PROJECT_SECRETS
            | zuse_protocol::methods::CLOUD_PROJECT_SECRET_SAVE
            | zuse_protocol::methods::CLOUD_PROJECT_SECRET_REVOKE => {
                let org = organization.clone();
                ensure!(
                    params["expectedAccountId"].as_str() == Some(account.as_str())
                        && params["expectedOrganizationId"].as_str() == org.as_deref()
                        && auth.user_id().as_deref() == Some(account.as_str()),
                    "Your account changed. Refresh repositories."
                );
                let mut input = params.clone();
                if let Some(object) = input.as_object_mut() {
                    object.remove("expectedAccountId");
                    object.remove("expectedOrganizationId");
                }
                let value = match method {
                    zuse_protocol::methods::CLOUD_CAPABILITIES => {
                        self.request_scoped(
                            &token,
                            &["v1", "cloud", "capabilities"],
                            None,
                            org.as_deref(),
                            &[],
                        )
                        .await?
                    }
                    zuse_protocol::methods::CLOUD_AGENT_CONNECTIONS => {
                        self.request_scoped(
                            &token,
                            &["v1", "cloud", "agents"],
                            None,
                            org.as_deref(),
                            &[],
                        )
                        .await?
                    }
                    zuse_protocol::methods::CLOUD_AGENT_CONFIGURE => {
                        let request: zuse_protocol::cloud_connections::ConfigureConnection =
                            serde_json::from_value(input)?;
                        self.request_scoped(
                            &token,
                            &["v1", "cloud", "agents", "configure"],
                            Some(serde_json::to_value(request)?),
                            org.as_deref(),
                            &[],
                        )
                        .await?
                    }
                    zuse_protocol::methods::CLOUD_AGENT_LOGIN => {
                        let request: zuse_protocol::cloud_connections::StartConnectionLogin =
                            serde_json::from_value(input)?;
                        self.request_scoped(
                            &token,
                            &["v1", "cloud", "agents", "login"],
                            Some(serde_json::to_value(request)?),
                            org.as_deref(),
                            &[],
                        )
                        .await?
                    }
                    zuse_protocol::methods::CLOUD_AGENT_LOGIN_STATUS
                    | zuse_protocol::methods::CLOUD_AGENT_LOGIN_CANCEL => {
                        let id = input["operationId"]
                            .as_str()
                            .filter(|s| !s.is_empty() && s.len() <= 200)
                            .ok_or_else(|| anyhow::anyhow!("Choose a pending agent login."))?;
                        self.request_scoped_verb(
                            &token,
                            &["v1", "cloud", "agents", "operations", id],
                            None,
                            org.as_deref(),
                            &[],
                            if method == zuse_protocol::methods::CLOUD_AGENT_LOGIN_CANCEL {
                                reqwest::Method::DELETE
                            } else {
                                reqwest::Method::GET
                            },
                        )
                        .await?
                    }
                    zuse_protocol::methods::CLOUD_AGENT_DISCONNECT => {
                        let id = input["connectionId"]
                            .as_str()
                            .filter(|s| !s.is_empty() && s.len() <= 200)
                            .ok_or_else(|| anyhow::anyhow!("Choose an agent connection."))?;
                        self.request_scoped_verb(
                            &token,
                            &["v1", "cloud", "agents", id],
                            None,
                            org.as_deref(),
                            &[],
                            reqwest::Method::DELETE,
                        )
                        .await?
                    }
                    zuse_protocol::methods::CLOUD_PROJECT_SECRETS
                    | zuse_protocol::methods::CLOUD_PROJECT_SECRET_SAVE
                    | zuse_protocol::methods::CLOUD_PROJECT_SECRET_REVOKE => {
                        let project = input["projectId"]
                            .as_str()
                            .filter(|id| !id.is_empty() && id.len() <= 200)
                            .ok_or_else(|| anyhow::anyhow!("Choose a project."))?
                            .to_owned();
                        let mut path = vec!["v1", "cloud", "projects", project.as_str(), "secrets"];
                        let save = method == zuse_protocol::methods::CLOUD_PROJECT_SECRET_SAVE;
                        let revoke = method == zuse_protocol::methods::CLOUD_PROJECT_SECRET_REVOKE;
                        let secret = input["secretId"].as_str().unwrap_or_default().to_owned();
                        let query = if revoke {
                            ensure!(
                                !secret.is_empty() && secret.len() <= 200,
                                "Choose a secret."
                            );
                            path.push(&secret);
                            vec![(
                                "expectedRevision".into(),
                                input["expectedRevision"]
                                    .as_i64()
                                    .ok_or_else(|| {
                                        anyhow::anyhow!(
                                            "Reload secret metadata before changing it."
                                        )
                                    })?
                                    .to_string(),
                            )]
                        } else {
                            vec![]
                        };
                        let body = if save {
                            let object = input
                                .as_object_mut()
                                .ok_or_else(|| anyhow::anyhow!("Invalid secret request."))?;
                            object.remove("projectId");
                            Some(input)
                        } else {
                            None
                        };
                        self.request_scoped_verb(
                            &token,
                            &path,
                            body,
                            org.as_deref(),
                            &query,
                            if revoke {
                                reqwest::Method::DELETE
                            } else if save {
                                reqwest::Method::POST
                            } else {
                                reqwest::Method::GET
                            },
                        )
                        .await?
                    }
                    zuse_protocol::methods::CLOUD_MACHINE
                    | zuse_protocol::methods::CLOUD_MACHINE_SAVE => {
                        let save = method == zuse_protocol::methods::CLOUD_MACHINE_SAVE;
                        let body = if save {
                            let request: zuse_protocol::cloud_computers::SaveMachine =
                                serde_json::from_value(input)?;
                            Some(serde_json::to_value(request)?)
                        } else {
                            None
                        };
                        self.request_scoped_verb(
                            &token,
                            &["v1", "cloud", "machine"],
                            body,
                            org.as_deref(),
                            &[],
                            if save {
                                reqwest::Method::PUT
                            } else {
                                reqwest::Method::GET
                            },
                        )
                        .await?
                    }
                    zuse_protocol::methods::CLOUD_PROJECT_ENVIRONMENT
                    | zuse_protocol::methods::CLOUD_PROJECT_ENVIRONMENT_SAVE => {
                        let project = input["projectId"]
                            .as_str()
                            .filter(|s| !s.is_empty() && s.len() <= 200)
                            .ok_or_else(|| anyhow::anyhow!("Choose a repository environment."))?
                            .to_owned();
                        let save = method == zuse_protocol::methods::CLOUD_PROJECT_ENVIRONMENT_SAVE;
                        let body = if save {
                            input.as_object_mut().unwrap().remove("projectId");
                            let request: zuse_protocol::cloud_computers::SaveEnvironment =
                                serde_json::from_value(input)?;
                            Some(serde_json::to_value(request)?)
                        } else {
                            None
                        };
                        self.request_scoped_verb(
                            &token,
                            &["v1", "cloud", "projects", &project, "environment"],
                            body,
                            org.as_deref(),
                            &[],
                            if save {
                                reqwest::Method::PUT
                            } else {
                                reqwest::Method::GET
                            },
                        )
                        .await?
                    }
                    zuse_protocol::methods::CLOUD_GITHUB_INSTALL => {
                        let action = input["action"].as_str().unwrap_or("start");
                        let (operation, body, query) = match action {
                            "start" => ("install", Some(json!({})), vec![]),
                            "status" | "cancel" | "select" => {
                                let id = input["operationId"]
                                    .as_str()
                                    .filter(|id| uuid::Uuid::parse_str(id).is_ok())
                                    .ok_or_else(|| {
                                        anyhow::anyhow!(
                                            "Refresh the GitHub connection and try again."
                                        )
                                    })?;
                                if action == "status" {
                                    ("status", None, vec![("operationId".into(), id.to_owned())])
                                } else if action == "select" {
                                    let installation = input["installationId"]
                                        .as_i64()
                                        .filter(|id| *id > 0)
                                        .ok_or_else(|| {
                                            anyhow::anyhow!("Choose a GitHub installation.")
                                        })?;
                                    (
                                        "select",
                                        Some(
                                            json!({"operationId":id,"installationId":installation}),
                                        ),
                                        vec![],
                                    )
                                } else {
                                    ("cancel", Some(json!({"operationId":id})), vec![])
                                }
                            }
                            "remove" => {
                                let installation = input["installationId"]
                                    .as_i64()
                                    .filter(|id| *id > 0)
                                    .ok_or_else(|| {
                                        anyhow::anyhow!("Choose a GitHub installation.")
                                    })?;
                                (
                                    "remove",
                                    Some(json!({"installationId":installation})),
                                    vec![],
                                )
                            }
                            "disconnect" => ("disconnect", Some(json!({})), vec![]),
                            _ => anyhow::bail!("Unknown GitHub connection action."),
                        };
                        self.request_scoped(
                            &token,
                            &["v1", "cloud", "github", operation],
                            body,
                            org.as_deref(),
                            &query,
                        )
                        .await?
                    }
                    zuse_protocol::methods::CLOUD_IMAGE => {
                        self.request_scoped(
                            &token,
                            &["v1", "cloud", "image"],
                            None,
                            org.as_deref(),
                            &[],
                        )
                        .await?
                    }
                    zuse_protocol::methods::CLOUD_IMAGE_DELETE => {
                        let request: zuse_protocol::cloud_projects::ImageDeleteRequest =
                            serde_json::from_value(params)?;
                        self.request_scoped(
                            &token,
                            &["v1", "cloud", "image", "delete"],
                            Some(serde_json::to_value(request)?),
                            org.as_deref(),
                            &[],
                        )
                        .await?
                    }
                    zuse_protocol::methods::CLOUD_IMAGE_BUILD => {
                        let request: zuse_protocol::cloud_projects::ImageBuildRequest =
                            serde_json::from_value(params)?;
                        self.request_scoped(
                            &token,
                            &["v1", "cloud", "image", "build"],
                            Some(serde_json::to_value(request)?),
                            org.as_deref(),
                            &[],
                        )
                        .await?
                    }
                    zuse_protocol::methods::CLOUD_PROJECT_CONNECT => {
                        let request: zuse_protocol::cloud_projects::ProjectConnectRequest =
                            serde_json::from_value(params)?;
                        self.request_scoped(
                            &token,
                            &["v1", "cloud", "projects"],
                            Some(serde_json::to_value(request)?),
                            org.as_deref(),
                            &[],
                        )
                        .await?
                    }
                    zuse_protocol::methods::CLOUD_PROJECT_REMOVE => {
                        let request: zuse_protocol::cloud_projects::ProjectRemoveRequest =
                            serde_json::from_value(params)?;
                        ensure!(
                            !request.project_id.is_empty() && request.project_id.len() <= 1024,
                            "Choose a repository to remove."
                        );
                        self.request_scoped_verb(
                            &token,
                            &["v1", "cloud", "projects", &request.project_id],
                            None,
                            org.as_deref(),
                            &[],
                            reqwest::Method::DELETE,
                        )
                        .await?
                    }
                    zuse_protocol::methods::CLOUD_GITHUB => {
                        self.request_scoped(
                            &token,
                            &["v1", "cloud", "github"],
                            None,
                            org.as_deref(),
                            &[],
                        )
                        .await?
                    }
                    _ => {
                        self.request_scoped(
                            &token,
                            &["v1", "cloud", "projects"],
                            None,
                            org.as_deref(),
                            &[],
                        )
                        .await?
                    }
                };
                json!({"accountId":account,"organizationId":org,"data":value})
            }
            zuse_protocol::methods::CLOUD_CONTEXT => {
                let value = self
                    .request_scoped(
                        &token,
                        &["v1", "cloud", "context"],
                        None,
                        organization.as_deref(),
                        &[],
                    )
                    .await?;
                json!({"accountId":account,"organizationId":organization,"ownerId":value["ownerId"],"apiOrigin":self.origin.as_str(),"projects":value["projects"],"agentCatalog":value["agentCatalog"],"providerAgentCatalogs":value["providerAgentCatalogs"],"computerCapabilities":value["computerCapabilities"],"commandAdmissionVersion":value["commandAdmissionVersion"],"agentConnections":value["agentConnections"]})
            }
            zuse_protocol::methods::CLOUD_CREATE => {
                let mut params = params;
                if let Some(expected) = params.get("expectedAccountId") {
                    ensure!(
                        expected.as_str() == Some(account.as_str())
                            && params["expectedOrganizationId"].as_str() == organization.as_deref(),
                        "The account changed. Select the cloud destination again."
                    );
                }
                if let Some(body) = params.as_object_mut() {
                    body.remove("attachmentTransfers");
                    body.remove("expectedAccountId");
                    body.remove("expectedOrganizationId");
                }
                let request: zuse_protocol::cloud::CloudWorkspaceCreateRequest =
                    serde_json::from_value(params)?;
                let value = self
                    .request_scoped(
                        &token,
                        &["v1", "cloud", "workspaces"],
                        Some(serde_json::to_value(request)?),
                        organization.as_deref(),
                        &[],
                    )
                    .await?;
                json!({"accountId":account,"organizationId":organization,"workspace":value})
            }
            LIST => {
                let value = self
                    .request_scoped(
                        &token,
                        &["v1", "cloud", "workspaces"],
                        None,
                        organization.as_deref(),
                        &[],
                    )
                    .await?;
                let mut rows: Vec<Workspace> = serde_json::from_value(value["workspaces"].clone())?;
                for row in &mut rows {
                    if organization.is_none() {
                        self.local_cleanup(&account, row, false);
                    }
                }
                rows.retain(|row| row.rpc_protocol.as_deref() == Some("zuse-rpc-v1"));
                json!({"accountId":account,"organizationId":organization,"apiOrigin":self.origin.as_str(),"workspaces":rows})
            }
            zuse_protocol::methods::CLOUD_COMPUTER_RECREATION => {
                self.computer_recreation(&token, &account, organization.as_deref(), &params)
                    .await?
            }
            zuse_protocol::methods::CLOUD_ALLOCATION_RETRY => {
                self.allocation_retry(&token, &account, organization.as_deref(), &params)
                    .await?
            }
            zuse_protocol::methods::CLOUD_RUNTIME_REPAIR => {
                self.runtime_repair(&token, &account, organization.as_deref(), &params)
                    .await?
            }
            zuse_protocol::methods::CLOUD_WORKSPACE => {
                let id = params["workspaceId"]
                    .as_str()
                    .filter(|id| !id.is_empty())
                    .ok_or_else(|| anyhow::anyhow!("Select a workspace."))?;
                ensure!(
                    params["expectedAccountId"].as_str() == Some(account.as_str())
                        && params["expectedOrganizationId"].as_str() == organization.as_deref(),
                    "The account changed. Select the workspace again."
                );
                let value = self
                    .request_scoped(
                        &token,
                        &["v1", "cloud", "workspaces", id],
                        None,
                        organization.as_deref(),
                        &[],
                    )
                    .await?;
                let logs = if params["includePreparationLogs"] == true {
                    self.request_scoped(
                        &token,
                        &["v1", "cloud", "workspaces", id, "preparation-logs"],
                        None,
                        organization.as_deref(),
                        &[],
                    )
                    .await?
                } else {
                    Value::Null
                };
                json!({"accountId":account,"organizationId":organization,"workspace":value.get("workspace").unwrap_or(&value),"preparationLogs":logs})
            }
            READ => 'history: {
                let id = params["workspaceId"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("Select a workspace."))?;
                let value = self
                    .request_scoped(
                        &token,
                        &["v1", "cloud", "workspaces", id],
                        None,
                        organization.as_deref(),
                        &[],
                    )
                    .await
                    .map_err(|error| {
                        if error
                            .downcast_ref::<CloudHttpFailure>()
                            .is_some_and(|failure| failure.0 == reqwest::StatusCode::NOT_FOUND)
                        {
                            // Only the workspace metadata lookup proves deletion. Missing
                            // projections, older pages and unpublished tools do not revoke it.
                            #[cfg(not(target_arch = "wasm32"))]
                            if !self.hosted {
                                let _ = clear_native_history_cache();
                            }
                            anyhow::Error::new(zuse_rpc::RpcError::Capability(
                                "workspace_not_found".into(),
                            ))
                        } else {
                            error
                        }
                    })?;
                let command_metadata = value.get("workspace").unwrap_or(&value).clone();
                let workspace: Workspace = serde_json::from_value(value["workspace"].clone())
                    .or_else(|_| serde_json::from_value(value.clone()))?;
                if let Some(reference) = params["toolBlobRef"].as_str() {
                    let offset = params["offset"]
                        .as_u64()
                        .filter(|n| *n <= 32 * 1024 * 1024)
                        .ok_or_else(|| anyhow::anyhow!("Invalid tool output cursor."))?;
                    let query = vec![
                        ("blobRef".into(), reference.to_owned()),
                        ("offset".into(), offset.to_string()),
                        ("limit".into(), "65536".into()),
                    ];
                    let output = self
                        .request_scoped(
                            &token,
                            &["v1", "cloud", "workspaces", id, "tool-output"],
                            None,
                            organization.as_deref(),
                            &query,
                        )
                        .await?;
                    break 'history json!({"accountId":account,"workspace":workspace,"commandDeliveryMode":command_metadata["commandDeliveryMode"],"commandAdmissionVersion":command_metadata["commandAdmissionVersion"],"queueRunSelectionVersion":command_metadata["queueRunSelectionVersion"],"taskCommandCanEdit":command_metadata["taskCommandCanEdit"],"toolOutput":output});
                }
                ensure!(
                    workspace.rpc_protocol.as_deref()
                        == Some(zuse_protocol::cloud_gateway::RUST_RPC),
                    "This workspace requires a runtime update."
                );
                let session = params["sessionId"]
                    .as_str()
                    .unwrap_or(&workspace.initial_session_id);
                let before = params["beforeSequence"].as_i64();
                ensure!(
                    before.is_none_or(|n| n > 0 && n < 9_007_199_254_740_991),
                    "Invalid history cursor."
                );
                let history = self.history.clone().with_scope(
                    &organization
                        .as_ref()
                        .map(|org| format!("organization:{org}"))
                        .unwrap_or_else(|| "personal".into()),
                )?;
                #[cfg(not(target_arch = "wasm32"))]
                let history = {
                    if self.hosted {
                        history
                    } else {
                        let root = std::env::var_os("ZUSE_CLOUD_HISTORY_CACHE_ROOT")
                            .map(std::path::PathBuf::from)
                            .or_else(|| {
                                std::env::var_os("HOME").map(|home| {
                                    std::path::PathBuf::from(home).join(".zuse/cache/cloud-history")
                                })
                            });
                        root.and_then(|root| history.with_page_cache(&root, &account).ok())
                            .unwrap_or(history)
                    }
                };
                let mut query = before
                    .map(|n| vec![("beforeSequence".into(), (n + 1).to_string())])
                    .unwrap_or_default();
                #[cfg(not(target_arch = "wasm32"))]
                query.push(("cacheProjection".into(), "true".into()));
                // This read has no provider dependency. An admitted command is
                // explicitly different from a guest transcript/cursor.
                if let Ok(saved) = self
                    .request_scoped(
                        &token,
                        &[
                            "v1",
                            "cloud",
                            "workspaces",
                            id,
                            "sessions",
                            session,
                            "task-history",
                        ],
                        None,
                        organization.as_deref(),
                        &query,
                    )
                    .await
                {
                    #[cfg(not(target_arch = "wasm32"))]
                    if saved["cacheProjection"].is_object() {
                        let _ = history.cache_projection(&token, &saved["cacheProjection"]);
                    }
                    if saved["source"] == "admission" {
                        let empty_admission = saved["admission"]["firstMessage"]
                            .as_str()
                            .is_none_or(|s| s.trim().is_empty())
                            && saved["admission"]["attachments"]
                                .as_array()
                                .is_none_or(|a| a.is_empty());
                        let head = if saved["session"].is_object() && empty_admission {
                            task_commands::registered_head(&workspace, session, &saved["session"])?
                        } else {
                            task_commands::admission_head(
                                &workspace,
                                &saved["admission"],
                                saved["acceptedAt"].as_i64().unwrap_or_default(),
                                saved["queuePaused"].as_bool().unwrap_or(false),
                            )?
                        };
                        break 'history json!({"head":head,"accountId":account,"workspace":workspace,"commandDeliveryMode":command_metadata["commandDeliveryMode"],"commandAdmissionVersion":command_metadata["commandAdmissionVersion"],"queueRunSelectionVersion":command_metadata["queueRunSelectionVersion"],"taskCommandCanEdit":command_metadata["taskCommandCanEdit"],"source":"admission","admission":saved["admission"],"session":saved["session"],"commandAdmissions":saved["commandAdmissions"],"registeredConversations":saved["registeredConversations"],"queuePaused":saved["queuePaused"],"acceptedAt":saved["acceptedAt"]});
                    }
                    if saved["source"] == "runtime"
                        && (params["incremental"] == true
                            || before.is_some()
                            || saved["head"]["olderMessageSequence"].is_null())
                    {
                        let head: Head = serde_json::from_value(saved["head"].clone())?;
                        ensure!(
                            before.is_none() || params["cursor"] == json!(head.cursor),
                            "Saved history changed. Reopen the conversation to refresh it."
                        );
                        break 'history json!({"accountId":account,"workspace":workspace,"commandDeliveryMode":command_metadata["commandDeliveryMode"],"commandAdmissionVersion":command_metadata["commandAdmissionVersion"],"queueRunSelectionVersion":command_metadata["queueRunSelectionVersion"],"taskCommandCanEdit":command_metadata["taskCommandCanEdit"],"head":head,"source":"postgres","commandAdmissions":saved["commandAdmissions"],"registeredConversations":saved["registeredConversations"],"queuePaused":saved["queuePaused"],"historyPage":before.is_some(),"requestedBefore":before});
                    }
                }
                let mut head: Head = history
                    .head(&token, id, session)
                    .await
                    .map_err(|_| anyhow::anyhow!("Saved cloud history is unavailable. Try again."))?
                    .ok_or_else(|| anyhow::anyhow!("Saved cloud history is not available yet."))?;
                if let Some(before) = before {
                    ensure!(
                        params["cursor"] == json!(head.cursor),
                        "Saved history changed. Reopen the conversation to refresh it."
                    );
                    let page = history.page(&token, &head, before).await?.ok_or_else(|| {
                        anyhow::anyhow!("Saved history is still being uploaded. Try again.")
                    })?;
                    head.transcript.replay_baseline =
                        Some(zuse_model::TranscriptBaseline::capture(&page.entries));
                    head.transcript.frame = zuse_model::TranscriptFrame::Reset {
                        reset: page.entries.into(),
                    };
                    head.older_message_sequence = page.older_message_sequence;
                    break 'history json!({"accountId":account,"workspace":workspace,"commandDeliveryMode":command_metadata["commandDeliveryMode"],"commandAdmissionVersion":command_metadata["commandAdmissionVersion"],"queueRunSelectionVersion":command_metadata["queueRunSelectionVersion"],"taskCommandCanEdit":command_metadata["taskCommandCanEdit"],"head":head,"historyPage":true,"requestedBefore":before});
                }
                if params["incremental"] == true {
                    break 'history json!({"accountId":account,"workspace":workspace,"commandDeliveryMode":command_metadata["commandDeliveryMode"],"commandAdmissionVersion":command_metadata["commandAdmissionVersion"],"queueRunSelectionVersion":command_metadata["queueRunSelectionVersion"],"taskCommandCanEdit":command_metadata["taskCommandCanEdit"],"head":head});
                }
                // Fully materialize saved pages for the existing transcript renderer, without opening an execution channel.
                let mut entries = match &head.transcript.frame {
                    zuse_model::TranscriptFrame::Reset { reset } => reset.as_ref().clone(),
                    _ => anyhow::bail!("Saved history is invalid."),
                };
                let mut before = head.older_message_sequence;
                let mut bytes = serde_json::to_vec(&entries)?.len();
                while let Some(n) = before {
                    let page = history.page(&token, &head, n).await?.ok_or_else(|| {
                        anyhow::anyhow!("Saved history is still being uploaded. Try again.")
                    })?;
                    before = page.older_message_sequence;
                    bytes += serde_json::to_vec(&page.entries)?.len();
                    ensure!(
                        bytes <= 64 * 1024 * 1024,
                        "This conversation is too large to open in this view."
                    );
                    let mut older = page.entries;
                    older.append(&mut entries);
                    entries = older;
                }
                head.transcript.replay_baseline =
                    Some(zuse_model::TranscriptBaseline::capture(&entries));
                head.transcript.frame = zuse_model::TranscriptFrame::Reset {
                    reset: entries.into(),
                };
                head.older_message_sequence = None;
                json!({"accountId":account,"workspace":workspace,"commandDeliveryMode":command_metadata["commandDeliveryMode"],"commandAdmissionVersion":command_metadata["commandAdmissionVersion"],"queueRunSelectionVersion":command_metadata["queueRunSelectionVersion"],"taskCommandCanEdit":command_metadata["taskCommandCanEdit"],"head":head})
            }
            CLEANUP_MIRROR => {
                ensure!(
                    organization.is_none(),
                    "Local folder sync is available for personal computers."
                );
                let id = params["workspaceId"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("Select a workspace."))?;
                let value = self
                    .request(&token, &["v1", "cloud", "workspaces", id], None)
                    .await?;
                let mut value = value.get("workspace").cloned().unwrap_or(value);
                let mut row: Workspace = serde_json::from_value(value.clone())?;
                ensure!(
                    matches!(row.state.as_str(), "archived" | "deleting" | "deleted"),
                    "The workspace must finish archiving before local cleanup."
                );
                self.local_cleanup(&account, &mut row, true);
                value["localMirrorCleanup"] = serde_json::to_value(row.local_mirror_cleanup)?;
                json!({"accountId": account, "workspace": value})
            }
            UNARCHIVE | DELETE => {
                let id = params["workspaceId"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("Select a workspace."))?;
                let command = params["commandId"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("Restore request is missing."))?;
                let value = self
                    .request_scoped(
                        &token,
                        &[
                            "v1",
                            "cloud",
                            "workspaces",
                            id,
                            if method == DELETE {
                                "delete"
                            } else {
                                "unarchive"
                            },
                        ],
                        Some(json!({"workspaceId":id,"commandId":command})),
                        organization.as_deref(),
                        &[],
                    )
                    .await?;
                let mut value = value;
                let mut workspace: Workspace = serde_json::from_value(value.clone())?;
                if organization.is_none() {
                    self.local_cleanup(&account, &mut workspace, false);
                }
                value["localMirrorCleanup"] = serde_json::to_value(workspace.local_mirror_cleanup)?;
                json!({"accountId":account,"workspace":value})
            }
            _ => anyhow::bail!("Unknown cloud operation."),
        };
        ensure!(
            auth.user_id().as_deref() == Some(&account)
                && auth.state().org_id() == organization.as_deref(),
            "Your account changed. Reopen this view."
        );
        if let Some(result) = result.as_object_mut() {
            result.insert("organizationId".into(), json!(organization));
            result.insert("apiOrigin".into(), json!(self.origin.as_str()));
        }
        #[cfg(not(target_arch = "wasm32"))]
        if !self.hosted && sidebar_cache::supports(method) {
            sidebar_cache::store(
                self.origin.as_str(),
                &account,
                organization.as_deref(),
                method,
                &result,
            );
        }
        Ok(result)
    }
}
pub async fn handle(auth: &Auth, method: &str, params: Value) -> Result<Value> {
    let api = CloudApi::shared()?;
    if matches!(
        method,
        zuse_protocol::methods::CLOUD_TASK_COMMAND | zuse_protocol::methods::CLOUD_TASK_ASSET
    ) {
        let auth = auth.clone();
        let method = method.to_owned();
        return tokio::spawn(async move{api.handle(&auth,&method,params).await}).await.map_err(|_|anyhow::anyhow!("The cloud operation was interrupted. Check its original identity before retrying."))?;
    }
    #[cfg(unix)]
    if auth.workos_enabled() || api.loopback {
        api.syncs.ensure_reconciliation(auth.clone());
    }
    api.handle(auth, method, params).await
}

#[cfg(test)]
mod beta_tests {
    use super::*;
    #[test]
    fn rejected_messages_say_why_instead_of_busy() {
        assert!(
            super::task_rejection("cloud_task_model_not_installed")
                .unwrap()
                .contains("model")
        );
        assert!(super::task_rejection("something_new").is_none());
    }
    #[test]
    fn an_outdated_computer_runtime_is_a_definitive_rejection_naming_the_fix() {
        let message =
            super::task_rejection(zuse_protocol::cloud_task_commands::RUNTIME_OUTDATED).unwrap();
        assert!(message.contains("older version"));
        assert!(
            !message.contains("recreate"),
            "recreation keeps the same runtime"
        );
        let rpc = TaskRejection(message).rpc();
        assert!(matches!(
            rpc,
            zuse_rpc::RpcError::Capability(code)
                if code == format!(
                    "{}{message}",
                    zuse_protocol::cloud_task_commands::TASK_REJECTED_PREFIX
                )
        ));
    }
    #[test]
    fn cloud_computers_are_off_in_production_unless_enabled() {
        assert!(!beta_enabled_for(None, "https://api.zuse.sh"));
        assert!(!beta_enabled_for(None, "https://api.zuse.sh/"));
        assert!(!beta_enabled_for(None, "https://api-rust.zuse.sh"));
        assert!(beta_enabled_for(None, "https://api-rust-stag.zuse.sh"));
        assert!(beta_enabled_for(None, "http://127.0.0.1:27655"));
        assert!(beta_enabled_for(Some("1"), "https://api.zuse.sh"));
        assert!(!beta_enabled_for(
            Some("0"),
            "https://api-rust-stag.zuse.sh"
        ));
    }
    #[test]
    fn the_account_and_billing_stay_outside_the_beta() {
        use zuse_protocol::methods;
        for method in [
            methods::CLOUD_CREATE,
            methods::CLOUD_WORKSPACES,
            methods::CLOUD_CONTEXT,
            methods::CLOUD_RPC,
            methods::CLOUD_SYNC_STATUS,
            methods::CLOUD_HISTORY,
        ] {
            assert!(beta_gated(method), "{method}");
        }
        for method in [
            methods::CLOUD_ACCOUNT_STATUS,
            methods::CLOUD_ACCOUNT_SIGN_IN,
            methods::CLOUD_BILLING_SUMMARY,
            methods::CLOUD_PREPAID_BALANCE,
            methods::ENGINE_INFO,
        ] {
            assert!(!beta_gated(method), "{method}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cloud_uses_the_account_api_origin() {
        assert_eq!(DEFAULT_API_URL, "https://api.zuse.sh");
        let api = CloudApi::new(DEFAULT_API_URL).unwrap();
        assert_eq!(api.origin.as_str(), "https://api.zuse.sh/");
    }
    #[test]
    fn credentials_require_a_valid_secure_cloud_origin() {
        for origin in [
            "http://api-rust.zuse.sh",
            "https://user:secret@api-rust.zuse.sh",
            "https://api-rust.zuse.sh/v1",
            "https://api-rust.zuse.sh/?token=secret",
        ] {
            assert!(CloudApi::new(origin).is_err());
        }
        assert!(CloudApi::new("https://api-rust.zuse.sh").is_ok());
        assert!(CloudApi::new("http://127.0.0.1:27655").is_ok());
    }
    #[tokio::test]
    async fn general_account_authorizes_cloud_and_retires_after_sign_out() {
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::TcpListener,
        };
        let dir = tempfile::tempdir().unwrap();
        let account = crate::account::Account::fixture_signed_in(
            dir.path(),
            crate::account::API_URL.into(),
            "general-account",
        );
        let identity = Auth::new(crate::auth::AuthConfig::new(
            "http://127.0.0.1:1",
            dir.path(),
        ))
        .with_account(Some(account.clone()));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let api = CloudApi::new(&format!("http://{}", listener.local_addr().unwrap())).unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut bytes = Vec::new();
            while !bytes.windows(4).any(|v| v == b"\r\n\r\n") {
                let mut buf = [0; 1024];
                let n = socket.read(&mut buf).await.unwrap();
                assert!(n > 0);
                bytes.extend_from_slice(&buf[..n]);
            }
            let headers = String::from_utf8(bytes).unwrap();
            assert!(headers.starts_with("GET /v1/cloud/projects "));
            assert!(
                headers
                    .to_ascii_lowercase()
                    .contains("authorization: bearer fixture.")
            );
            assert!(!headers.contains("dev-user"));
            let body = r#"{"projects":[]}"#;
            socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body).as_bytes()).await.unwrap();
        });
        let value = api
            .handle(
                &identity,
                zuse_protocol::methods::CLOUD_PROJECTS,
                json!({"expectedAccountId":"general-account"}),
            )
            .await
            .unwrap();
        assert_eq!(value["accountId"], "general-account");
        assert!(value["organizationId"].is_null());
        server.await.unwrap();
        let mut status = identity.status_stream();
        use futures::StreamExt;
        assert_eq!(
            status.next().await.unwrap()["user"]["id"],
            "general-account"
        );
        let mut changes = identity.watch_state();
        account.sign_out().unwrap();
        let signed_out = status.next().await.unwrap();
        assert!(signed_out["user"].is_null());
        assert!(signed_out["orgId"].is_null());
        changes.changed().await.unwrap();
        assert!(identity.user_id().is_none());
        assert!(identity.access_token().await.is_err());
        let catalog = api.handle(&identity, LIST, json!({})).await.unwrap();
        assert!(catalog["accountId"].is_null());
        assert!(
            api.handle(
                &identity,
                zuse_protocol::methods::CLOUD_BILLING_SUMMARY,
                json!({})
            )
            .await
            .unwrap_err()
            .to_string()
            .contains("Sign in")
        );
    }
    #[tokio::test]
    async fn account_switch_fences_old_repository_actions() {
        let dir = tempfile::tempdir().unwrap();
        let account = crate::account::Account::fixture_signed_in(
            dir.path(),
            crate::account::API_URL.into(),
            "first",
        );
        let identity = Auth::new(crate::auth::AuthConfig::new(
            "http://127.0.0.1:1",
            dir.path(),
        ))
        .with_account(Some(account.clone()));
        let mut changes = identity.watch_state();
        account.fixture_install("second");
        changes.changed().await.unwrap();
        assert_eq!(identity.user_id().as_deref(), Some("second"));
        let api = CloudApi::new("http://127.0.0.1:1").unwrap();
        let error = api
            .handle(
                &identity,
                zuse_protocol::methods::CLOUD_PROJECT_REMOVE,
                json!({"expectedAccountId":"first","projectId":"project"}),
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("account changed"));
        let error = api
            .handle(
                &identity,
                zuse_protocol::methods::CLOUD_BILLING_CAP,
                json!({"expectedAccountId":"first","overageCapMicros":0,"idempotencyKey":"old"}),
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("account changed"));
    }
    #[tokio::test]
    async fn local_profiles_never_send_synthetic_identity_to_production() {
        let dir = tempfile::tempdir().unwrap();
        let auth = Auth::new(crate::auth::AuthConfig::new(
            "http://127.0.0.1:1",
            dir.path(),
        ));
        let api = CloudApi::new("https://api-rust.zuse.sh").unwrap();
        assert_eq!(
            api.handle(&auth, LIST, json!({})).await.unwrap()["workspaces"],
            json!([])
        );
        assert!(
            api.handle(&auth, READ, json!({"workspaceId":"other-account"}))
                .await
                .is_err()
        );
        assert!(
            api.handle(
                &auth,
                UNARCHIVE,
                json!({"workspaceId":"other-account","commandId":"command"})
            )
            .await
            .is_err()
        );
    }
}

/// Reconcile reference mirrors at boot/login even when the archived screen is closed.
/// Only the metadata catalog is read: this loop cannot allocate or resume compute.
pub struct MirrorReconciler(tokio::task::JoinHandle<()>);
impl MirrorReconciler {
    pub fn abort(&self) {
        self.0.abort();
    }
    pub async fn stop(&mut self) {
        self.0.abort();
        let _ = (&mut self.0).await;
    }
}
impl Drop for MirrorReconciler {
    fn drop(&mut self) {
        self.0.abort();
    }
}
pub fn start_mirror_reconciliation(auth: Auth) -> MirrorReconciler {
    MirrorReconciler(tokio::spawn(async move {
        use futures::StreamExt;
        let mut changes = auth.watch_state();
        loop {
            if !auth.state().is_signed_in() {
                if changes.changed().await.is_err() {
                    break;
                }
                continue;
            }
            let Ok(api) = CloudApi::shared() else { break };
            let account = auth.user_id();
            let organization = auth.state().org_id().map(str::to_owned);
            #[cfg(unix)]
            let _ = sync::reconcile(api.clone(), auth.clone()).await;
            let _ = api.handle(&auth, LIST, json!({})).await;
            let stream = events::handle(
                &auth,
                json!({"expectedAccountId":account,"expectedOrganizationId":organization}),
            )
            .await;
            let Ok(zuse_rpc::RpcReply::Stream(mut stream)) = stream else {
                if changes.changed().await.is_err() {
                    break;
                }
                continue;
            };
            let mut legacy = false;
            let mut recovery = tokio::time::interval(Duration::from_secs(30));
            recovery.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                let refresh = tokio::select! {
                    result=changes.changed()=>{if result.is_err(){return;}break;},
                    _=recovery.tick(),if legacy=>true,
                    value=stream.next()=>{
                        let Some(value)=value else{break};
                        if value["accountId"].as_str()!=account.as_deref() || value["organizationId"].as_str()!=organization.as_deref(){continue;}
                        match value["data"]["type"].as_str(){
                            Some("disabled")=>{legacy=true;true},
                            Some("connected")=>{legacy=false;true},
                            Some("reset"|"revalidate")=>true,
                            Some("events")=>value["data"]["events"].as_array().is_some_and(|events|events.iter().any(|e|e["kind"]=="workspace")) || value["data"]["revokedWorkspaceIds"].as_array().is_some_and(|ids|!ids.is_empty()),
                            _=>false,
                        }
                    }
                };
                if refresh
                    && auth.user_id() == account
                    && auth.state().org_id() == organization.as_deref()
                {
                    #[cfg(unix)]
                    let _ = sync::reconcile(api.clone(), auth.clone()).await;
                    let _ = api.handle(&auth, LIST, json!({})).await;
                }
            }
        }
    }))
}

#[cfg(test)]
mod definitive_create_rejection_tests {
    use super::*;
    #[test]
    fn explicit_rejection_is_distinct_from_uncertain_transport_or_generic_conflict() {
        assert_eq!(
            definitive_create_rejection(CREATE_LIMIT_REJECTED),
            Some("staging_candidate_computer_limit")
        );
        assert_eq!(
            definitive_create_rejection(CREATE_REPOSITORY_REJECTED),
            Some("github_repository_access_denied")
        );
        assert_eq!(
            definitive_create_rejection(CREATE_GITHUB_REJECTED),
            Some("github_runtime_credentials_required")
        );
        for uncertain in [
            "The workspace is busy. Wait for its current operation to finish.",
            "Cloud service is unavailable. Try again.",
            "connection closed",
            "transport: timed out",
        ] {
            assert!(definitive_create_rejection(uncertain).is_none());
        }
    }
}

#[cfg(test)]
mod context_failure_tests {
    use super::*;
    #[tokio::test]
    async fn expired_compute_context_is_not_reported_as_busy_workspace() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let router = axum::Router::new()
            .route(
                "/v1/cloud/context",
                axum::routing::get(|| async {
                    (
                        axum::http::StatusCode::CONFLICT,
                        axum::Json(json!({"error":"staging_managed_base_candidate_expired"})),
                    )
                }),
            )
            .route(
                "/v1/cloud/projects",
                axum::routing::get(|| async {
                    (
                        axum::http::StatusCode::FORBIDDEN,
                        axum::Json(json!({"error":"workspace_access_denied"})),
                    )
                }),
            );
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let api = CloudApi::new(&format!("http://{address}")).unwrap();
        let error = api
            .request_scoped(
                "fixture-token",
                &["v1", "cloud", "context"],
                None,
                None,
                &[],
            )
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("setup expired"), "{error}");
        assert!(!error.contains("workspace is busy"), "{error}");
        let error = api
            .request_scoped(
                "fixture-token",
                &["v1", "cloud", "projects"],
                None,
                None,
                &[],
            )
            .await
            .unwrap_err()
            .to_string();
        assert_eq!(error, "You do not have access to this workspace.");
        server.abort();
    }
}
