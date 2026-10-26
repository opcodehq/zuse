//! Host-authorized control plane access. No access token crosses the viewport RPC boundary.
pub mod environment;
pub mod launch;
pub mod identity;
#[cfg(unix)]
pub mod local_terminal;
#[cfg(unix)]
pub mod mirror;
#[cfg(unix)]
pub mod sync;
// The cloud modules share this account authority, independently of workspace login.
use identity::CloudIdentity as Auth;
use anyhow::{Result, ensure};
use serde_json::{Value, json};
use std::{
    sync::{Arc, OnceLock},
    time::Duration,
};
use zuse_model::cloud::{Head, Workspace};

pub const LIST: &str = zuse_protocol::methods::CLOUD_WORKSPACES;
pub const READ: &str = zuse_protocol::methods::CLOUD_HISTORY;
pub const CLEANUP_MIRROR: &str = zuse_protocol::methods::CLOUD_CLEANUP_MIRROR;
pub const DELETE: &str = zuse_protocol::methods::CLOUD_DELETE;
pub const UNARCHIVE: &str = zuse_protocol::methods::CLOUD_UNARCHIVE;
pub fn handles(method: &str) -> bool {
    matches!(
        method,
        LIST | READ
            | zuse_protocol::methods::CLOUD_LOCAL_TERMINAL
            | zuse_protocol::methods::CLOUD_SYNC_STATUS
            | zuse_protocol::methods::CLOUD_SYNC_CONFIGURE
            | zuse_protocol::methods::CLOUD_SYNC_RETRY
            | zuse_protocol::methods::CLOUD_PAUSE
            | zuse_protocol::methods::CLOUD_WORKSPACE
            | zuse_protocol::methods::CLOUD_IMAGE
            | zuse_protocol::methods::CLOUD_IMAGE_DELETE
            | zuse_protocol::methods::CLOUD_IMAGE_BUILD
            | zuse_protocol::methods::CLOUD_PROJECTS
            | zuse_protocol::methods::CLOUD_PROJECT_CONNECT
            | zuse_protocol::methods::CLOUD_PROJECT_REMOVE
            | zuse_protocol::methods::CLOUD_GITHUB
            | zuse_protocol::methods::CLOUD_CONTEXT
            | zuse_protocol::methods::CLOUD_CREATE
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
    origin: reqwest::Url,
    http: reqwest::Client,
    history: zuse_cloud_projection::reader::Reader,
    loopback: bool,
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
            origin,
            http: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(Duration::from_secs(20))
                .build()?,
            history,
            loopback,
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
    fn shared() -> Result<Arc<Self>> {
        static API: OnceLock<Result<Arc<CloudApi>, String>> = OnceLock::new();
        API.get_or_init(|| {
            Self::new(
                &std::env::var("ZUSE_CLOUD_API_URL")
                    .unwrap_or_else(|_| "https://api-rust.zuse.sh".into()),
            )
            .map(Arc::new)
            .map_err(|_| "Cloud service configuration is invalid.".into())
        })
        .clone()
        .map_err(anyhow::Error::msg)
    }
    fn local_cleanup(&self, account: &str, row: &mut Workspace, retry: bool) {
        let admitted = matches!(row.state.as_str(), "archived" | "deleting" | "deleted");
        #[cfg(unix)]
        {
            self.local_terminals.observe(account, row);
            row.local_mirror_cleanup =
                self.mirrors
                    .observe(account, &row.workspace_id, admitted, retry);
            self.syncs.observe(account,row);
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
            if path
                .get(2)
                .is_some_and(|p| matches!(*p, "launch-assets" | "workspaces"))
            {
                let error: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
                let actionable = match error["error"].as_str() {
                    Some("cloud_launch_attachments_runtime_update_required") => Some(
                        "Rebuild this repository's cloud image before launching with attachments.",
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
                        Some("cloud_entitlement_required") =>
                            "Subscribe to Cloud before adding repositories.",
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
            ensure!(
                status.is_success(),
                "{}",
                match status.as_u16() {
                    401 => "Sign in to access cloud workspaces.",
                    403 => "You do not have access to this workspace.",
                    409 => "The workspace is busy. Wait for its current operation to finish.",
                    _ => "Cloud service is unavailable. Try again.",
                }
            );
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
            ensure!(expected.as_str() == Some(account.as_str())
                && params["expectedOrganizationId"].as_str() == auth.state().org_id(),
                "Your account changed. Reopen this view.");
        }
        let token = auth
            .access_token()
            .await
            .map_err(|_| anyhow::anyhow!("Sign in to access cloud workspaces."))?;
        ensure!(auth.user_id().as_deref() == Some(account.as_str())
            && auth.state().org_id() == organization.as_deref(),
            "Your account changed. Reopen this view.");
        #[cfg(unix)]
        if matches!(method,LIST|READ|DELETE|UNARCHIVE|CLEANUP_MIRROR|zuse_protocol::methods::CLOUD_WORKSPACE) {
            self.syncs.hydrate(self,auth,&account)?;
        }
        let result = match method {
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
            | zuse_protocol::methods::CLOUD_GITHUB => {
                let org = organization.clone();
                ensure!(
                    params["expectedAccountId"].as_str() == Some(account.as_str())
                        && params["expectedOrganizationId"].as_str() == org.as_deref()
                        && auth.user_id().as_deref() == Some(account.as_str()),
                    "Your account changed. Refresh repositories."
                );
                let value = match method {
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
                    .request(&token, &["v1", "cloud", "context"], None)
                    .await?;
                json!({"accountId":account,"projects":value["projects"],"agentCatalog":value["agentCatalog"]})
            }
            zuse_protocol::methods::CLOUD_CREATE => {
                let mut params = params;
                if let Some(expected) = params.get("expectedAccountId") {
                    ensure!(
                        expected.as_str() == Some(account.as_str())
                            && auth.state().org_id().is_none(),
                        "The account changed. Select the cloud destination again."
                    );
                }
                if let Some(body) = params.as_object_mut() {
                    body.remove("attachmentTransfers");
                    body.remove("expectedAccountId");
                }
                let request: zuse_protocol::cloud::CloudWorkspaceCreateRequest =
                    serde_json::from_value(params)?;
                let value = self
                    .request(
                        &token,
                        &["v1", "cloud", "workspaces"],
                        Some(serde_json::to_value(request)?),
                    )
                    .await?;
                json!({"accountId":account,"workspace":value})
            }
            LIST => {
                let value = self
                    .request(&token, &["v1", "cloud", "workspaces"], None)
                    .await?;
                let mut rows: Vec<Workspace> = serde_json::from_value(value["workspaces"].clone())?;
                for row in &mut rows {
                    self.local_cleanup(&account, row, false);
                }
                rows.retain(|row| row.rpc_protocol.as_deref() == Some("zuse-rpc-v1"));
                json!({"accountId":account,"workspaces":rows})
            }
            zuse_protocol::methods::CLOUD_WORKSPACE => {
                let id = params["workspaceId"]
                    .as_str()
                    .filter(|id| !id.is_empty())
                    .ok_or_else(|| anyhow::anyhow!("Select a workspace."))?;
                ensure!(
                    params["expectedAccountId"].as_str() == Some(account.as_str())
                        && auth.state().org_id().is_none(),
                    "The account changed. Select the workspace again."
                );
                let value = self
                    .request(&token, &["v1", "cloud", "workspaces", id], None)
                    .await?;
                json!({"accountId":account,"workspace":value.get("workspace").unwrap_or(&value)})
            }
            READ => {
                let id = params["workspaceId"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("Select a workspace."))?;
                let value = self
                    .request(&token, &["v1", "cloud", "workspaces", id], None)
                    .await?;
                let workspace: Workspace = serde_json::from_value(value["workspace"].clone())
                    .or_else(|_| serde_json::from_value(value.clone()))?;
                ensure!(
                    workspace.rpc_protocol.as_deref()
                        == Some(zuse_protocol::cloud_gateway::RUST_RPC),
                    "This workspace requires a runtime update."
                );
                let session = params["sessionId"]
                    .as_str()
                    .unwrap_or(&workspace.initial_session_id);
                let mut head: Head = self
                    .history
                    .head(&token, id, session)
                    .await
                    .map_err(|_| anyhow::anyhow!("Saved cloud history is unavailable. Try again."))?
                    .ok_or_else(|| anyhow::anyhow!("Saved cloud history is not available yet."))?;
                // Fully materialize saved pages for the existing transcript renderer, without opening an execution channel.
                let mut entries = match &head.transcript.frame {
                    zuse_model::TranscriptFrame::Reset { reset } => reset.clone(),
                    _ => anyhow::bail!("Saved history is invalid."),
                };
                let mut before = head.older_message_sequence;
                let mut bytes = serde_json::to_vec(&entries)?.len();
                while let Some(n) = before {
                    let page = self.history.page(&token, &head, n).await?.ok_or_else(|| {
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
                head.transcript.frame = zuse_model::TranscriptFrame::Reset { reset: entries };
                head.older_message_sequence = None;
                json!({"accountId":account,"workspace":workspace,"head":head})
            }
            CLEANUP_MIRROR => {
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
                    .request(
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
                    )
                    .await?;
                let mut value = value;
                let mut workspace: Workspace = serde_json::from_value(value.clone())?;
                self.local_cleanup(&account, &mut workspace, false);
                value["localMirrorCleanup"] = serde_json::to_value(workspace.local_mirror_cleanup)?;
                json!({"accountId":account,"workspace":value})
            }
            _ => anyhow::bail!("Unknown cloud operation."),
        };
        ensure!(
            auth.user_id().as_deref() == Some(&account) && auth.state().org_id() == organization.as_deref(),
            "Your account changed. Reopen this view."
        );
        Ok(result)
    }
}
pub async fn handle(auth: &Auth, method: &str, params: Value) -> Result<Value> {
    let api=CloudApi::shared()?;
    #[cfg(unix)]
    if auth.workos_enabled()||api.loopback {api.syncs.ensure_reconciliation(auth.clone());}
    api.handle(auth, method, params).await
}

#[cfg(test)]
mod tests {
    use super::*;
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
        use tokio::{io::{AsyncReadExt, AsyncWriteExt}, net::TcpListener};
        let dir = tempfile::tempdir().unwrap();
        let account = crate::account::Account::fixture_signed_in(
            dir.path(), crate::account::API_URL.into(), "general-account");
        let identity = Auth::new(crate::auth::AuthConfig::new(
            "http://127.0.0.1:1", dir.path())).with_account(Some(account.clone()));
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
            assert!(headers.to_ascii_lowercase().contains("authorization: bearer fixture."));
            assert!(!headers.contains("dev-user"));
            let body = r#"{"projects":[]}"#;
            socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body).as_bytes()).await.unwrap();
        });
        let value = api.handle(&identity, zuse_protocol::methods::CLOUD_PROJECTS,
            json!({"expectedAccountId":"general-account"})).await.unwrap();
        assert_eq!(value["accountId"], "general-account");
        assert!(value["organizationId"].is_null());
        server.await.unwrap();
        let mut status = identity.status_stream();
        use futures::StreamExt;
        assert_eq!(status.next().await.unwrap()["user"]["id"], "general-account");
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
        assert!(api.handle(&identity, zuse_protocol::methods::CLOUD_BILLING_SUMMARY,
            json!({})).await.unwrap_err().to_string().contains("Sign in"));
    }
    #[tokio::test]
    async fn account_switch_fences_old_repository_actions() {
        let dir = tempfile::tempdir().unwrap();
        let account = crate::account::Account::fixture_signed_in(
            dir.path(), crate::account::API_URL.into(), "first");
        let identity = Auth::new(crate::auth::AuthConfig::new(
            "http://127.0.0.1:1", dir.path())).with_account(Some(account.clone()));
        let mut changes = identity.watch_state();
        account.fixture_install("second");
        changes.changed().await.unwrap();
        assert_eq!(identity.user_id().as_deref(), Some("second"));
        let api = CloudApi::new("http://127.0.0.1:1").unwrap();
        let error = api.handle(&identity, zuse_protocol::methods::CLOUD_PROJECT_REMOVE,
            json!({"expectedAccountId":"first","projectId":"project"})).await.unwrap_err();
        assert!(error.to_string().contains("account changed"));
        let error = api.handle(&identity, zuse_protocol::methods::CLOUD_BILLING_CAP,
            json!({"expectedAccountId":"first","overageCapMicros":0,"idempotencyKey":"old"})).await.unwrap_err();
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
        let mut changes = auth.watch_state();
        loop {
            if auth.state().is_signed_in() {
                if let Ok(api) = CloudApi::shared() {
                    #[cfg(unix)]
                    let _ = sync::reconcile(api.clone(),auth.clone()).await;
                    let _ = api.handle(&auth, LIST, json!({})).await;
                }
            }
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(30)) => {},
                result = changes.changed() => if result.is_err() { break; },
            }
        }
    }))
}
