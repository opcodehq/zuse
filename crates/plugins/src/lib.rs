//! Account-bound client for the existing managed Executor service.
//! No vendor credentials or catalog endpoints cross into a UI or agent session.

use async_trait::async_trait;
use futures::StreamExt;
use reqwest::{Client, Url};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::{sync::Arc, time::Duration};
use tokio_util::sync::CancellationToken;
use zuse_protocol::plugins::{PluginRequest, PluginResponse, PluginTenant, PluginToolRequest};

pub mod local;

const MAX_RESPONSE_BYTES: usize = 4 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum PluginError {
    #[error("Sign in to connect and use plugins.")]
    SignedOut,
    #[error("This plugin session belongs to another account. Start a new session.")]
    AccountChanged,
    #[error("Plugin request was cancelled.")]
    Cancelled,
    #[error("Plugin service is unavailable. Check the account connection and try again.")]
    Unavailable,
    #[error("Plugin service rejected this request ({0}).")]
    Rejected(u16),
    #[error("Plugin service returned an invalid response.")]
    InvalidResponse,
    #[error("Plugin service URL must be an HTTPS base URL.")]
    InvalidEndpoint,
    #[error("Choose a valid plugin workspace belonging to this account.")]
    InvalidWorkspace,
    #[error("Plugin calls require session permission.")]
    PermissionRequired,
    #[error("{0}")]
    Local(&'static str),
}

/// A snapshot from the account authority. Token refresh belongs to that authority.
pub struct PluginCredential {
    pub subject: String,
    pub access_token: String,
}

#[async_trait]
pub trait PluginCredentials: Send + Sync {
    async fn current(&self) -> Result<PluginCredential, PluginError>;
}

#[derive(Clone)]
pub struct PluginClient {
    http: Client,
    endpoint: Url,
    credentials: Arc<dyn PluginCredentials>,
    subject: Arc<str>,
}

impl PluginClient {
    pub async fn new(
        endpoint: &str,
        credentials: Arc<dyn PluginCredentials>,
    ) -> Result<Self, PluginError> {
        let endpoint = Url::parse(endpoint).map_err(|_| PluginError::InvalidEndpoint)?;
        if endpoint.scheme() != "https"
            || endpoint.host_str().is_none()
            || !endpoint.username().is_empty()
            || endpoint.password().is_some()
            || endpoint.query().is_some()
            || endpoint.fragment().is_some()
            || endpoint.path().trim_matches('/').len() != 0
        {
            return Err(PluginError::InvalidEndpoint);
        }
        let current = credentials.current().await?;
        if current.subject.is_empty() || current.access_token.is_empty() {
            return Err(PluginError::SignedOut);
        }
        let http = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(60))
            .build()
            .map_err(|_| PluginError::Unavailable)?;
        Ok(Self {
            http,
            endpoint,
            credentials,
            subject: current.subject.into(),
        })
    }

    pub async fn request(
        &self,
        request: &PluginRequest,
        cancel: &CancellationToken,
    ) -> Result<PluginResponse, PluginError> {
        let tenant = request.tenant_id();
        let response = self.post("/v1/plugins", tenant, request, cancel).await?;
        if let PluginResponse::Snapshot { tenant_id, .. } = &response {
            if tenant.is_some_and(|expected| expected != tenant_id) {
                return Err(PluginError::InvalidResponse);
            }
        }
        Ok(response)
    }

    /// The gateway passes a permission receipt for a call only after its session
    /// policy accepts that tool invocation. Discovery never invokes a tool.
    pub async fn tools(
        &self,
        request: &PluginToolRequest,
        permitted: bool,
        cancel: &CancellationToken,
    ) -> Result<serde_json::Value, PluginError> {
        self.tools_in_tenant(
            &format!("personal:{}", self.subject),
            request,
            permitted,
            cancel,
        )
        .await
    }

    /// Tenant is bound by the engine session, never supplied by an agent tool.
    pub async fn tools_in_tenant(
        &self,
        tenant: &str,
        request: &PluginToolRequest,
        permitted: bool,
        cancel: &CancellationToken,
    ) -> Result<serde_json::Value, PluginError> {
        if request.invokes_external_tool() && !permitted {
            return Err(PluginError::PermissionRequired);
        }
        self.post("/v1/plugins/tools", Some(tenant), request, cancel)
            .await
    }

    /// Discover content workspaces using the hosted account's membership API.
    pub async fn workspaces(
        &self,
        cancel: &CancellationToken,
    ) -> Result<Vec<PluginTenant>, PluginError> {
        #[derive(Deserialize)]
        struct Organization {
            id: String,
            name: String,
            role: String,
        }
        let read = async {
            let credential = self.credentials.current().await?;
            if credential.subject != self.subject.as_ref() {
                return Err(PluginError::AccountChanged);
            }
            if credential.access_token.is_empty() {
                return Err(PluginError::SignedOut);
            }
            let response = self
                .http
                .get(
                    self.endpoint
                        .join("/v1/organizations")
                        .map_err(|_| PluginError::InvalidEndpoint)?,
                )
                .bearer_auth(credential.access_token)
                .send()
                .await
                .map_err(|_| PluginError::Unavailable)?;
            let organizations: Vec<Organization> = match self.decode(response).await {
                // Older deployments or disabled organization rollout still support Personal.
                Err(PluginError::Rejected(403 | 404)) => vec![],
                result => result?,
            };
            if organizations.len() > 1024 {
                return Err(PluginError::InvalidResponse);
            }
            let mut tenants = vec![PluginTenant {
                id: format!("personal:{}", self.subject),
                kind: "personal".into(),
                name: "Personal".into(),
            }];
            for org in organizations
                .into_iter()
                .filter(|org| matches!(org.role.as_str(), "admin" | "member"))
            {
                let id = format!("organization:{}", org.id);
                scoped_route("/v1/plugins", Some(&id), &self.subject)?;
                if !tenants.iter().any(|tenant| tenant.id == id) {
                    tenants.push(PluginTenant {
                        id,
                        kind: "organization".into(),
                        name: org.name,
                    });
                }
            }
            if self.credentials.current().await?.subject != self.subject.as_ref() {
                return Err(PluginError::AccountChanged);
            }
            Ok(tenants)
        };
        tokio::select! { biased; _ = cancel.cancelled() => Err(PluginError::Cancelled), result = read => result }
    }

    async fn post<I: Serialize + ?Sized, O: DeserializeOwned>(
        &self,
        path: &str,
        tenant: Option<&str>,
        input: &I,
        cancel: &CancellationToken,
    ) -> Result<O, PluginError> {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => Err(PluginError::Cancelled),
            result = self.post_inner(path, tenant, input) => result,
        }
    }

    async fn post_inner<I: Serialize + ?Sized, O: DeserializeOwned>(
        &self,
        path: &str,
        tenant: Option<&str>,
        input: &I,
    ) -> Result<O, PluginError> {
        let credentials = self.credentials.current().await?;
        if credentials.subject != self.subject.as_ref() {
            return Err(PluginError::AccountChanged);
        }
        if credentials.access_token.is_empty() {
            return Err(PluginError::SignedOut);
        }
        let (path, workspace) = scoped_route(path, tenant, &self.subject)?;
        let url = self
            .endpoint
            .join(&path)
            .map_err(|_| PluginError::InvalidEndpoint)?;
        let response = self
            .http
            .post(url)
            .bearer_auth(credentials.access_token)
            .header("x-zuse-workspace", workspace)
            .json(input)
            .send()
            .await
            .map_err(|_| PluginError::Unavailable)?;
        self.decode(response).await
    }

    async fn decode<O: DeserializeOwned>(
        &self,
        response: reqwest::Response,
    ) -> Result<O, PluginError> {
        if !response.status().is_success() {
            return Err(PluginError::Rejected(response.status().as_u16()));
        }
        if response
            .content_length()
            .is_some_and(|n| n > MAX_RESPONSE_BYTES as u64)
        {
            return Err(PluginError::InvalidResponse);
        }
        let mut body = Vec::new();
        let mut chunks = response.bytes_stream();
        while let Some(chunk) = chunks.next().await {
            let chunk = chunk.map_err(|_| PluginError::Unavailable)?;
            if body.len().saturating_add(chunk.len()) > MAX_RESPONSE_BYTES {
                return Err(PluginError::InvalidResponse);
            }
            body.extend_from_slice(&chunk);
        }
        // Account switching while an upstream request was in flight must not
        // publish another account's discovery results into the new UI context.
        if self.credentials.current().await?.subject != self.subject.as_ref() {
            return Err(PluginError::AccountChanged);
        }
        serde_json::from_slice(&body).map_err(|_| PluginError::InvalidResponse)
    }
}

/// Versioned routes prevent an older API from ignoring organization scope and
/// silently executing a tool against Personal. The API still authorizes membership.
fn scoped_route(
    path: &str,
    tenant: Option<&str>,
    subject: &str,
) -> Result<(String, String), PluginError> {
    match tenant {
        None => Ok((path.into(), "personal".into())),
        Some(tenant) if tenant.strip_prefix("personal:") == Some(subject) => {
            Ok((path.into(), "personal".into()))
        }
        Some(tenant) => {
            let id = tenant
                .strip_prefix("organization:")
                .ok_or(PluginError::InvalidWorkspace)?;
            if id.is_empty()
                || id.len() > 128
                || !id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
            {
                return Err(PluginError::InvalidWorkspace);
            }
            Ok((
                format!("/v1/organization-workspaces/{id}{path}"),
                tenant.into(),
            ))
        }
    }
}

#[cfg(test)]
mod tests;
