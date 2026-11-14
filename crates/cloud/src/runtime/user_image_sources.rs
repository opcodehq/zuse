//! Fetch tokens for repositories baked into user images. The worker holds no
//! GitHub user authority: it asks the API, with a short-lived proof bound to
//! one building image and one project, and the API mints a token scoped to
//! that single repository from the account owner's own GitHub connection.
//! Only personal accounts bake private repositories (see
//! `user_images::repositories::select`).
use super::*;
use crate::project_caches::user_images::{BuildPlan, repository_source::RepositoryCredentials};
use std::sync::Weak;
use zeroize::Zeroizing;

pub(crate) struct WorkerImageAuthority {
    runtime: Weak<RuntimeControl>,
    http: reqwest::Client,
}
impl WorkerImageAuthority {
    pub(crate) fn new(runtime: Weak<RuntimeControl>) -> Result<Self, ApiError> {
        Ok(Self {
            runtime,
            http: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(std::time::Duration::from_secs(20))
                .build()
                .map_err(|_| ApiError::unavailable("user_image_source_authority_unavailable"))?,
        })
    }
}
fn scope(image: &str) -> String {
    format!("user-image:{image}")
}

#[async_trait::async_trait]
impl RepositoryCredentials for WorkerImageAuthority {
    async fn token(&self, image_id: &str, project_id: &str) -> Result<Zeroizing<String>, ApiError> {
        let unavailable = || ApiError::unavailable("user_image_source_authority_unavailable");
        let runtime = self.runtime.upgrade().ok_or_else(unavailable)?;
        let now = chrono::Utc::now().timestamp_millis();
        let proof = runtime.signer.allocation_scope_proof(
            &scope(image_id),
            &format!("project:{project_id}"),
            1,
            now,
        )?;
        let mut url = reqwest::Url::parse(&runtime.signer.issuer()).map_err(|_| unavailable())?;
        if url.scheme() != "https" || !url.username().is_empty() || url.password().is_some() {
            return Err(unavailable());
        }
        url.path_segments_mut()
            .map_err(|_| unavailable())?
            .clear()
            .extend([
                "v1",
                "cloud",
                "machine",
                "images",
                image_id,
                "repositories",
                project_id,
                "source-credential",
            ]);
        let mut response = self
            .http
            .post(url)
            .bearer_auth(proof)
            .send()
            .await
            .map_err(|_| unavailable())?;
        let status = response.status();
        if !status.is_success() {
            return Err(if status.is_server_error() {
                unavailable()
            } else {
                ApiError::forbidden("user_image_source_access_denied")
            });
        }
        let mut bytes = Zeroizing::new(Vec::new());
        while let Some(chunk) = response.chunk().await.map_err(|_| unavailable())? {
            if bytes.len() + chunk.len() > 16 * 1024 {
                return Err(unavailable());
            }
            bytes.extend_from_slice(&chunk);
        }
        #[derive(serde::Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Grant {
            token: String,
            expires_at: i64,
        }
        let grant: Grant = serde_json::from_slice(&bytes).map_err(|_| unavailable())?;
        let token = Zeroizing::new(grant.token);
        if token.is_empty()
            || token.contains(['\n', '\r', '\0', ' '])
            || grant.expires_at <= chrono::Utc::now().timestamp_millis() + 30_000
        {
            return Err(unavailable());
        }
        Ok(token)
    }
}

impl RuntimeControl {
    /// API side: a token for one private repository of an image that is in its
    /// repository phase right now, scoped by GitHub to that repository and
    /// minted from the owner's own connection. Never stored.
    pub async fn user_image_source_credential(
        &self,
        image: &str,
        project: &str,
        proof: &str,
        now: i64,
    ) -> Result<Value, ApiError> {
        let denied = || ApiError::forbidden("user_image_source_scope_denied");
        if !crate::project_caches::user_images::image_id_valid(image) {
            return Err(denied());
        }
        let claims = self
            .signer
            .verify_allocation_scope(proof, &scope(image), now)?;
        if claims["operationId"] != format!("project:{project}") {
            return Err(denied());
        }
        let row: Option<(String, Value)> = sqlx::query_as("SELECT owner_id,plan FROM cloud_machine_images WHERE image_id=$1 AND state='building' AND phase='repositories'")
            .bind(image)
            .fetch_optional(&self.pool)
            .await?;
        let (owner, plan) = row.ok_or_else(denied)?;
        let plan: BuildPlan = serde_json::from_value(plan).map_err(|_| denied())?;
        plan.validate().map_err(|_| denied())?;
        let repository = plan
            .repositories
            .iter()
            .find(|r| r.project_id == project && r.visibility == "private")
            .ok_or_else(denied)?;
        if owner != plan.identity.owner_id || owner.starts_with("organization:") {
            return Err(denied());
        }
        let current: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM api_cloud_projects WHERE account_id=$1 AND project_id=$2 AND included=true AND visibility='private' AND lower(repository_identity)=lower($3))")
            .bind(&owner)
            .bind(project)
            .bind(&repository.repository_identity)
            .fetch_one(&self.pool)
            .await?;
        if !current {
            return Err(denied());
        }
        let authority = self
            .scope_authority
            .get()
            .and_then(|a| a.github.as_ref())
            .ok_or_else(|| ApiError::unavailable("user_image_source_authority_unavailable"))?;
        let name = repository
            .repository_identity
            .strip_prefix("github.com/")
            .ok_or_else(denied)?;
        let grant = authority.scoped_git(&owner, &owner, name).await?;
        Ok(json!({"token":grant.token.as_str(),"expiresAt":grant.authority_expires_at}))
    }
}
