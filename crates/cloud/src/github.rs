//! Read-through migration adapter. Existing GitHub OAuth/installation authority stays upstream.
use crate::{access::Access, error::ApiError};
use zuse_protocol::cloud_projects::GithubCatalog;
pub struct LegacyGithub {
    url: reqwest::Url,
    http: reqwest::Client,
}
impl LegacyGithub {
    pub fn new(origin: &str) -> anyhow::Result<Self> {
        let mut url = reqwest::Url::parse(origin)?;
        anyhow::ensure!(
            url.scheme() == "https"
                && url.username().is_empty()
                && url.password().is_none()
                && url.query().is_none()
                && url.fragment().is_none(),
            "GitHub authority must be an HTTPS origin"
        );
        url.set_path("/v1/cloud/github");
        Ok(Self {
            url,
            http: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(std::time::Duration::from_secs(20))
                .build()?,
        })
    }
    #[cfg(test)]
    pub(crate) fn test_url(&mut self, url: reqwest::Url) {
        self.url = url;
    }
    pub async fn catalog(&self, access: &Access, bearer: &str) -> Result<GithubCatalog, ApiError> {
        let mut request = self.http.get(self.url.clone()).bearer_auth(bearer);
        let scope = match &access.scope {
            crate::access::Scope::Personal => "personal".into(),
            crate::access::Scope::Organization { organization_id } => {
                format!("organization:{organization_id}")
            }
        };
        request = request.header("x-zuse-workspace", scope);
        let mut response = request.send().await.map_err(|_| unavailable())?;
        if !response.status().is_success() {
            return Err(unavailable());
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| unavailable())? {
            if bytes.len() + chunk.len() > 8 * 1024 * 1024 {
                return Err(unavailable());
            }
            bytes.extend_from_slice(&chunk);
        }
        let mut result: GithubCatalog =
            serde_json::from_slice(&bytes).map_err(|_| unavailable())?;
        // Permissions come from the freshly verified local scope, never upstream response hints.
        result.can_manage_installations = Some(
            access
                .membership
                .as_ref()
                .is_none_or(|m| m.role.slug == "admin"),
        );
        Ok(result)
    }
}
fn unavailable() -> ApiError {
    ApiError::unavailable("cloud_github_unavailable")
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Json, Router, http::HeaderMap, routing::get};
    use serde_json::{Value, json};
    #[tokio::test]
    async fn scoped_legacy_catalog_filters_private_fields_and_refuses_redirects() {
        async fn catalog(headers: HeaderMap) -> Json<Value> {
            assert_eq!(headers["authorization"], "Bearer existing-workos-token");
            assert_eq!(headers["x-zuse-workspace"], "organization:org-team");
            Json(
                json!({"configured":true,"canManageInstallations":true,"installations":[],"repositories":[],"privateToken":"MUST-NOT-LEAVE-AUTHORITY"}),
            )
        }
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(
                listener,
                Router::new().route("/catalog", get(catalog)).route(
                    "/redirect",
                    get(|| async { axum::response::Redirect::temporary("/catalog") }),
                ),
            )
            .await
            .unwrap()
        });
        let access = Access {
            actor: crate::auth::Principal {
                account_id: "member".into(),
            },
            scope: crate::access::Scope::Organization {
                organization_id: "org-team".into(),
            },
            owner_id: "organization:org-team".into(),
            membership: Some(crate::auth::Membership {
                id: "member".into(),
                user_id: "member".into(),
                organization_id: "org-team".into(),
                status: "active".into(),
                role: crate::auth::MembershipRole {
                    slug: "member".into(),
                },
            }),
        };
        let mut reader = LegacyGithub::new("https://api.zuse.sh").unwrap();
        reader.test_url(format!("http://{address}/catalog").parse().unwrap());
        let result = reader
            .catalog(&access, "existing-workos-token")
            .await
            .unwrap();
        assert_eq!(result.can_manage_installations, Some(false));
        assert!(!serde_json::to_string(&result).unwrap().contains("MUST-NOT"));
        reader.test_url(format!("http://{address}/redirect").parse().unwrap());
        assert_eq!(
            reader
                .catalog(&access, "existing-workos-token")
                .await
                .err()
                .unwrap()
                .code,
            "cloud_github_unavailable"
        );
        assert!(LegacyGithub::new("http://api.zuse.sh").is_err());
        assert!(LegacyGithub::new("https://user:secret@api.zuse.sh").is_err());
        server.abort();
    }
}
