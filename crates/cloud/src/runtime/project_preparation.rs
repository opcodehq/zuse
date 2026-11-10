//! API-only membership authority; worker proofs are not browser/runtime tickets.
mod allocation_source;
use super::*;
use crate::project_caches::worker::{Authorization, Request};
use std::sync::{Arc, Weak};
pub(crate) struct WorkerAuthority {
    runtime: Weak<RuntimeControl>,
    http: reqwest::Client,
}
#[async_trait::async_trait]
impl Authorization for WorkerAuthority {
    async fn authorize(&self, request: &Request) -> Result<(), ApiError> {
        let response = self.call(request, "authorize", None, None).await?;
        if response.status() != reqwest::StatusCode::NO_CONTENT {
            return Err(ApiError::forbidden(
                "project_preparation_authorization_revoked",
            ));
        }
        Ok(())
    }
}
impl WorkerAuthority {
    async fn call(
        &self,
        request: &Request,
        action: &str,
        github_user: Option<i64>,
        source_authorization: Option<&str>,
    ) -> Result<reqwest::Response, ApiError> {
        let runtime = self
            .runtime
            .upgrade()
            .ok_or_else(|| ApiError::unavailable("project_preparation_authority_unavailable"))?;
        let now = chrono::Utc::now().timestamp_millis();
        let proof = runtime.signer.allocation_scope_proof(
            &format!("preparation:{}", request.request_id),
            &format!("project:{}", request.project),
            request.environment_revision,
            now,
        )?;
        let mut url = reqwest::Url::parse(&runtime.signer.issuer())
            .map_err(|_| ApiError::unavailable("project_preparation_authority_unavailable"))?;
        if url.scheme() != "https"
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err(ApiError::unavailable(
                "project_preparation_authority_unavailable",
            ));
        }
        url.path_segments_mut()
            .map_err(|_| ApiError::unavailable("project_preparation_authority_unavailable"))?
            .clear()
            .extend([
                "v1",
                "cloud",
                "projects",
                &request.project,
                "environment",
                "preparations",
                &request.request_id,
                action,
            ]);
        if let Some(user) = github_user {
            url.query_pairs_mut()
                .append_pair("githubUserId", &user.to_string());
        }
        let request = self.http.post(url).bearer_auth(proof);
        let request = if let Some(authorization) = source_authorization {
            request.header("x-zuse-project-source-proof", authorization)
        } else {
            request
        };
        let response = request
            .send()
            .await
            .map_err(|_| ApiError::unavailable("project_preparation_authority_unavailable"))?;
        Ok(response)
    }
}
#[async_trait::async_trait]
impl crate::project_caches::source::SourceAuthority for WorkerAuthority {
    async fn authorize_request(&self, request: &Request) -> Result<(), ApiError> {
        Authorization::authorize(self, request).await
    }

    async fn credential(
        &self,
        request: &Request,
    ) -> Result<crate::project_caches::source::SourceCredential, ApiError> {
        let mut response = self.call(request, "source-credential", None, None).await?;
        if response.status() != reqwest::StatusCode::OK {
            return Err(ApiError::forbidden(
                "project_preparation_authorization_revoked",
            ));
        }
        let mut bytes = zeroize::Zeroizing::new(Vec::new());
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| ApiError::unavailable("project_cache_source_authority_unavailable"))?
        {
            if bytes.len() + chunk.len() > 32 * 1024 {
                return Err(ApiError::unavailable("project_cache_source_grant_invalid"));
            }
            bytes.extend_from_slice(&chunk);
        }
        #[derive(serde::Deserialize)]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        struct Grant {
            token: String,
            github_user_id: i64,
            expires_at: i64,
            authority_expires_at: i64,
            provider_expires_at: i64,
            authorization: String,
        }
        let grant: Grant = serde_json::from_slice(&bytes)
            .map_err(|_| ApiError::unavailable("project_cache_source_grant_invalid"))?;
        let credential = crate::project_caches::source::SourceCredential {
            token: zeroize::Zeroizing::new(grant.token),
            github_user_id: grant.github_user_id,
            expires_at: grant.expires_at,
            provider_expires_at: grant.provider_expires_at,
            authorization: grant.authorization,
        };
        if grant.authority_expires_at != credential.expires_at
            || credential.provider_expires_at < credential.expires_at
            || credential.github_user_id <= 0
            || credential.token.is_empty()
            || credential.expires_at <= chrono::Utc::now().timestamp_millis()
        {
            return Err(ApiError::forbidden(
                "project_cache_source_credential_expired",
            ));
        }
        Ok(credential)
    }
    async fn revalidate(
        &self,
        request: &Request,
        credential: &crate::project_caches::source::SourceCredential,
    ) -> Result<(), ApiError> {
        let response = self
            .call(
                request,
                "authorize",
                Some(credential.github_user_id),
                Some(&credential.authorization),
            )
            .await?;
        if response.status() != reqwest::StatusCode::NO_CONTENT {
            return Err(ApiError::forbidden(
                "project_preparation_authorization_revoked",
            ));
        }
        Ok(())
    }
}
impl crate::project_caches::source::SourceCipher for WorkerAuthority {
    fn seal(&self, key: &str, bytes: &[u8]) -> Result<Vec<u8>, ApiError> {
        let runtime = self
            .runtime
            .upgrade()
            .ok_or_else(|| ApiError::unavailable("project_cache_source_key_unavailable"))?;
        runtime
            .master
            .seal(format!("zuse-project-source-v1\n{key}").as_bytes(), bytes)
            .map(String::into_bytes)
            .map_err(|_| ApiError::unavailable("project_cache_source_encryption_failed"))
    }
    fn open(&self, key: &str, bytes: &[u8]) -> Result<Vec<u8>, ApiError> {
        let runtime = self
            .runtime
            .upgrade()
            .ok_or_else(|| ApiError::unavailable("project_cache_source_key_unavailable"))?;
        let text = std::str::from_utf8(bytes)
            .map_err(|_| ApiError::unavailable("project_cache_source_invalid"))?;
        runtime
            .master
            .open(format!("zuse-project-source-v1\n{key}").as_bytes(), text)
            .map(|data| data.to_vec())
            .map_err(|_| ApiError::unavailable("project_cache_source_invalid"))
    }
}
impl RuntimeControl {
    pub async fn configure_public_project_preparation(self: &Arc<Self>) -> Result<(), ApiError> {
        let enabled = std::env::var("ZUSE_CLOUD_PROJECT_PREPARATION_ENABLED").as_deref() == Ok("1");
        if !self.project_cache_control.available().await? {
            return if enabled {
                Err(ApiError::unavailable("project_cache_schema_not_ready"))
            } else {
                Ok(())
            };
        }
        let (boxd_owned,source_owned):(bool,bool)=sqlx::query_as("SELECT EXISTS(SELECT 1 FROM zuse_rust_project_cache_operations WHERE provider='boxd' AND state IN('queued','running','uncertain','cleaning','ready')),EXISTS(SELECT 1 FROM zuse_rust_project_source_bundles)").fetch_one(&self.pool).await?;
        if !enabled && !boxd_owned && !source_owned {
            return Ok(());
        }
        let objects = self
            .transcript_objects
            .clone()
            .ok_or_else(|| ApiError::unavailable("project_cache_objects_not_configured"))?;
        let authority = Arc::new(WorkerAuthority {
            runtime: Arc::downgrade(self),
            http: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(std::time::Duration::from_secs(5))
                .build()
                .map_err(|_| ApiError::unavailable("project_preparation_authority_unavailable"))?,
        });
        let bundles: Arc<dyn crate::project_caches::source::Bundles> = Arc::new(
            if enabled {
                crate::project_caches::source::PublicBundles::new(self.pool.clone(), objects)?
            } else {
                crate::project_caches::source::PublicBundles::for_cleanup(
                    self.pool.clone(),
                    objects,
                )?
            }
            .with_source_authority(authority.clone())
            .with_cipher(authority.clone()),
        );
        let mut backend = crate::project_caches::backends::Registry::new(bundles.clone());
        if enabled || boxd_owned {
            let key = std::env::var("BOXD_API_KEY")
                .map_err(|_| ApiError::unavailable("project_preparation_boxd_not_configured"))?;
            let mut provider =
                crate::providers::boxd::Boxd::new(key, std::env::var("BOXD_ORG").ok())
                    .map_err(|_| ApiError::unavailable("project_preparation_provider_invalid"))?;
            if let Ok(base) = std::env::var("BOXD_BASE_URL") {
                provider = provider
                    .with_base_url(&base)
                    .map_err(|_| ApiError::unavailable("project_preparation_provider_invalid"))?;
            }
            backend.register(
                zuse_protocol::cloud_computers::ProviderId::Boxd,
                Arc::new(crate::project_caches::boxd::BoxdPreparation::new(
                    self.pool.clone(),
                    Arc::new(provider),
                    bundles,
                )),
            )?;
        }
        self.project_cache_control
            .install_mode(authority, Arc::new(backend), enabled)
    }
    pub async fn authorize_project_preparation(
        &self,
        project: &str,
        request: &str,
        proof: &str,
        now: i64,
    ) -> Result<(), ApiError> {
        self.authorize_project_preparation_user(project, request, proof, now, None)
            .await
    }
    pub async fn authorize_project_preparation_user(
        &self,
        project: &str,
        request: &str,
        proof: &str,
        now: i64,
        expected_user: Option<i64>,
    ) -> Result<(), ApiError> {
        let record = self
            .project_preparation_record(project, request, proof, now)
            .await?;
        let visibility: String = sqlx::query_scalar(
            "SELECT visibility FROM api_cloud_projects WHERE account_id=$1 AND project_id=$2",
        )
        .bind(record["owner_id"].as_str())
        .bind(project)
        .fetch_one(&self.pool)
        .await?;
        if visibility == "private" || expected_user.is_some() {
            let authority = self
                .scope_authority
                .get()
                .and_then(|a| a.github.as_ref())
                .ok_or_else(|| {
                    ApiError::unavailable("project_cache_source_authority_unavailable")
                })?;
            let owner = required(&record, "owner_id")?;
            let actor = required(&record, "actor_id")?;
            let url = format!("https://{}.git", required(&record, "repository_identity")?);
            let before = authority.connected_user_id(actor).await?;
            if expected_user.is_some_and(|expected| expected != before) {
                return Err(ApiError::forbidden("github_account_changed"));
            }
            authority.authorize_repository(owner, actor, &url).await?;
            if authority.connected_user_id(actor).await? != before {
                return Err(ApiError::forbidden("github_account_changed"));
            }
            // Membership/revision may have changed during upstream observation.
            self.project_preparation_record(
                project,
                request,
                proof,
                chrono::Utc::now().timestamp_millis(),
            )
            .await?;
        }
        Ok(())
    }
    pub async fn project_preparation_source_credential(
        &self,
        project: &str,
        request: &str,
        proof: &str,
        now: i64,
    ) -> Result<Value, ApiError> {
        let record = self
            .project_preparation_record(project, request, proof, now)
            .await?;
        let authority = self
            .scope_authority
            .get()
            .and_then(|a| a.github.as_ref())
            .ok_or_else(|| ApiError::unavailable("project_cache_source_authority_unavailable"))?;
        let owner = required(&record, "owner_id")?;
        let actor = required(&record, "actor_id")?;
        let repository = required(&record, "repository_identity")?
            .strip_prefix("github.com/")
            .ok_or_else(|| ApiError::conflict("project_cache_repository_invalid"))?;
        let url = format!("https://github.com/{repository}.git");
        let authorization = authority.authorize_repository(owner, actor, &url).await?;
        let grant = authority.scoped_git(owner, actor, repository).await?;
        self.authorize_project_preparation_user(
            project,
            request,
            proof,
            chrono::Utc::now().timestamp_millis(),
            Some(grant.github_user_id),
        )
        .await?;
        let mut tx = self.pool.begin().await?;
        authority
            .revalidate_repository(&mut tx, &authorization, &url)
            .await?;
        tx.commit().await?;
        let encoded = serde_json::to_vec(&authorization)
            .map_err(|_| ApiError::unavailable("project_cache_source_grant_invalid"))?;
        let sealed = self
            .master
            .seal(&source_aad(&record), &encoded)
            .map_err(|_| ApiError::unavailable("project_cache_source_grant_invalid"))?;
        use base64::Engine;
        let sealed = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(sealed);
        Ok(
            json!({"token":grant.token.as_str(), "githubUserId":grant.github_user_id, "expiresAt":grant.authority_expires_at, "authorityExpiresAt":grant.authority_expires_at, "providerExpiresAt":grant.provider_expires_at, "authorization":sealed}),
        )
    }
    pub async fn revalidate_project_source(
        &self,
        project: &str,
        request: &str,
        proof: &str,
        now: i64,
        github_user: i64,
        source_authorization: &str,
    ) -> Result<(), ApiError> {
        if source_authorization.len() > 16 * 1024 {
            return Err(ApiError::forbidden("project_cache_source_grant_invalid"));
        }
        let record = self
            .project_preparation_record(project, request, proof, now)
            .await?;
        use base64::Engine;
        let sealed = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(source_authorization)
            .map_err(|_| ApiError::forbidden("project_cache_source_grant_invalid"))?;
        let sealed = std::str::from_utf8(&sealed)
            .map_err(|_| ApiError::forbidden("project_cache_source_grant_invalid"))?;
        let bytes = self
            .master
            .open(&source_aad(&record), sealed)
            .map_err(|_| ApiError::forbidden("project_cache_source_grant_invalid"))?;
        let authorization: crate::github::native::RepositoryAuthorization =
            serde_json::from_slice(&bytes)
                .map_err(|_| ApiError::forbidden("project_cache_source_grant_invalid"))?;
        self.authorize_project_preparation_user(project, request, proof, now, Some(github_user))
            .await?;
        let authority = self
            .scope_authority
            .get()
            .and_then(|a| a.github.as_ref())
            .ok_or_else(|| ApiError::unavailable("project_cache_source_authority_unavailable"))?;
        let url = format!("https://{}.git", required(&record, "repository_identity")?);
        let mut tx = self.pool.begin().await?;
        authority
            .revalidate_repository(&mut tx, &authorization, &url)
            .await?;
        tx.commit().await?;
        Ok(())
    }
    async fn project_preparation_record(
        &self,
        project: &str,
        request: &str,
        proof: &str,
        now: i64,
    ) -> Result<Value, ApiError> {
        if !self.project_cache_control.available().await? {
            return Err(ApiError::unavailable("project_cache_schema_not_ready"));
        }
        let claims =
            self.signer
                .verify_allocation_scope(proof, &format!("preparation:{request}"), now)?;
        if claims["operationId"] != format!("project:{project}") {
            return Err(ApiError::forbidden("project_preparation_scope_denied"));
        }
        let record:Option<Value>=sqlx::query_scalar("SELECT to_jsonb(r) FROM zuse_rust_project_preparation_requests r WHERE r.request_id=$1 AND r.project_id=$2").bind(request).bind(project).fetch_optional(&self.pool).await?;
        let record = record.ok_or_else(|| ApiError::not_found("project_preparation_not_found"))?;
        let owner = required(&record, "owner_id")?;
        let actor = required(&record, "actor_id")?;
        if record["environment_revision"] != claims["generation"]
            || !self.creation_accounts.iter().any(|id| id == owner)
        {
            return Err(ApiError::forbidden("project_preparation_scope_denied"));
        }
        if owner.starts_with("organization:") {
            let access = self.scope_actor(owner, actor).await?;
            let membership = access
                .membership
                .as_ref()
                .ok_or_else(|| ApiError::forbidden("organization_access_denied"))?;
            if record["actor"]["membershipId"] != membership.id
                || !matches!(membership.role.slug.as_str(), "admin" | "owner")
            {
                return Err(ApiError::forbidden("project_preparation_scope_denied"));
            }
        } else if owner != actor || !record["actor"].is_null() {
            return Err(ApiError::forbidden("project_preparation_scope_denied"));
        }
        let current:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM api_cloud_projects p WHERE p.account_id=$1 AND p.project_id=$2 AND p.included=true AND p.visibility IN ('public','private') AND p.repository_identity=$3 AND p.state='connected' AND coalesce(p.secret_bindings,'[]'::jsonb)='[]'::jsonb) AND $4=(SELECT max(revision) FROM cloud_environment_versions WHERE owner_id=$1 AND project_id=$2)").bind(owner).bind(project).bind(record["repository_identity"].as_str()).bind(record["environment_revision"].as_i64()).fetch_one(&self.pool).await?;
        if !current {
            return Err(ApiError::forbidden("project_preparation_scope_denied"));
        }
        if let Some(authority) = crate::project_caches::pilot::current(now)? {
            let request = crate::project_caches::worker::Request {
                request_id: request.into(),
                owner: owner.into(),
                actor_id: actor.into(),
                actor: record["actor"].clone(),
                project: project.into(),
                environment_revision: record["environment_revision"]
                    .as_i64()
                    .ok_or_else(|| ApiError::forbidden("project_preparation_scope_denied"))?,
                requested_ref: required(&record, "requested_ref")?.into(),
                repository_identity: required(&record, "repository_identity")?.into(),
            };
            let mut tx = self.pool.begin().await?;
            authority.request_tx(&mut tx, &request, now).await?;
            tx.commit().await?;
        }
        Ok(record)
    }
}

fn source_aad(record: &Value) -> Vec<u8> {
    format!(
        "zuse-project-source-authority-v1\n{}\n{}\n{}\n{}\n{}",
        record["request_id"],
        record["project_id"],
        record["environment_revision"],
        record["owner_id"],
        record["actor_id"]
    )
    .into_bytes()
}
