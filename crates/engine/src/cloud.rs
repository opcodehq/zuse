//! Host-authorized control plane access. No access token crosses the viewport RPC boundary.
pub mod environment;
#[cfg(unix)]
pub mod mirror;
use crate::auth::Auth;
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
            | zuse_protocol::methods::CLOUD_CONTEXT
            | zuse_protocol::methods::CLOUD_CREATE
            | zuse_protocol::methods::CLOUD_BILLING_SUMMARY
            | zuse_protocol::methods::CLOUD_BILLING_USAGE
            | zuse_protocol::methods::CLOUD_BILLING_CAP
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
            row.local_mirror_cleanup =
                self.mirrors
                    .observe(account, &row.workspace_id, admitted, retry);
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
        let mut url = self.origin.clone();
        url.path_segments_mut()
            .map_err(|_| anyhow::anyhow!("Cloud service configuration is invalid."))?
            .clear()
            .extend(path.iter().copied());
        let request = if let Some(body) = body {
            self.http.post(url).json(&body)
        } else {
            self.http.get(url)
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
            if path.starts_with(&["v1", "cloud", "billing"]) || path == ["v1", "billing", "portal"]
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
                Ok(json!({"accountId":auth.user_id(),"workspaces":[]}))
            } else {
                anyhow::bail!("Sign in to access cloud workspaces.")
            };
        }
        let account = auth
            .user_id()
            .ok_or_else(|| anyhow::anyhow!("Sign in to access cloud workspaces."))?;
        let token = auth
            .access_token()
            .await
            .map_err(|_| anyhow::anyhow!("Sign in to access cloud workspaces."))?;
        let result = match method {
            zuse_protocol::methods::CLOUD_BILLING_PORTAL => {
                let org = auth.state().org_id().map(str::to_owned);
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
                let org = auth.state().org_id().map(str::to_owned);
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
            zuse_protocol::methods::CLOUD_CONTEXT => {
                let value = self
                    .request(&token, &["v1", "cloud", "context"], None)
                    .await?;
                json!({"accountId":account,"projects":value["projects"]})
            }
            zuse_protocol::methods::CLOUD_CREATE => {
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
            auth.user_id().as_deref() == Some(&account),
            "Your account changed. Reopen this view."
        );
        Ok(result)
    }
}
pub async fn handle(auth: &Auth, method: &str, params: Value) -> Result<Value> {
    CloudApi::shared()?.handle(auth, method, params).await
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
            if auth.user_id().is_some() {
                if let Ok(api) = CloudApi::shared() {
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
