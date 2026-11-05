//! Private source metadata stays behind the API-owned actor/GitHub authority.
use super::*;
use crate::project_caches::ref_resolution::ResolvedSource;
impl RuntimeControl {
    pub(crate) async fn resolve_private_project_source(
        &self,
        workspace: &Value,
        operation: &Value,
    ) -> Result<ResolvedSource, ApiError> {
        let id = required(workspace, "workspace_id")?;
        let proof = self.signer.allocation_scope_proof(
            id,
            required(operation, "operation_id")?,
            operation["generation"]
                .as_i64()
                .ok_or_else(|| ApiError::conflict("project_cache_allocation_fenced"))?,
            chrono::Utc::now().timestamp_millis(),
        )?;
        let mut url = reqwest::Url::parse(&self.signer.issuer())
            .map_err(|_| ApiError::unavailable("project_cache_source_authority_unavailable"))?;
        url.path_segments_mut()
            .map_err(|_| ApiError::unavailable("project_cache_source_authority_unavailable"))?
            .clear()
            .extend(["v1", "cloud", "workspaces", id, "allocation-source"]);
        let mut response = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(std::time::Duration::from_secs(20))
            .build()
            .map_err(|_| ApiError::unavailable("project_cache_source_authority_unavailable"))?
            .post(url)
            .bearer_auth(proof)
            .send()
            .await
            .map_err(|_| ApiError::unavailable("project_cache_source_authority_unavailable"))?;
        if !response.status().is_success() {
            return Err(if response.status().is_client_error() {
                ApiError::forbidden("project_cache_source_authorization_revoked")
            } else {
                ApiError::unavailable("project_cache_source_authority_unavailable")
            });
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| ApiError::unavailable("project_cache_source_response_invalid"))?
        {
            if bytes.len() + chunk.len() > 512 * 1024 {
                return Err(ApiError::unavailable(
                    "project_cache_source_response_invalid",
                ));
            }
            bytes.extend_from_slice(&chunk);
        }
        serde_json::from_slice(&bytes)
            .map_err(|_| ApiError::unavailable("project_cache_source_response_invalid"))
    }
    async fn private_project_source_binding(
        &self,
        id: &str,
        proof: &str,
        now: i64,
    ) -> Result<(Value, Value), ApiError> {
        let claims = self.signer.verify_allocation_scope(proof, id, now)?;
        let row: Value = sqlx::query_scalar(
            "SELECT to_jsonb(w) FROM api_cloud_workspaces w WHERE workspace_id=$1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| ApiError::not_found("cloud_workspace_not_found"))?;
        if blocked(&row)
            || row["desired_state"] != "ready"
            || row["request_config"]["rustAllocationOperationId"] != claims["operationId"]
            || row["request_config"]["runtimeGeneration"] != claims["generation"]
        {
            return Err(ApiError::forbidden("project_cache_allocation_fenced"));
        }
        let planned: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM zuse_rust_allocations WHERE operation_id=$1 AND workspace_id=$2 AND generation=$3 AND phase='planned' AND machine_id IS NULL)")
            .bind(claims["operationId"].as_str()).bind(id).bind(claims["generation"].as_i64()).fetch_one(&self.pool).await?;
        if !planned {
            return Err(ApiError::forbidden("project_cache_allocation_fenced"));
        }
        self.candidate_row_allowed(&row, now).await?;
        self.scope_access(id, None, true).await?;
        let owner = required(&row, "account_id")?;
        let project = required(&row, "project_id")?;
        let metadata: Value = sqlx::query_scalar("SELECT to_jsonb(p) FROM api_cloud_projects p WHERE account_id=$1 AND project_id=$2 AND included=true AND state='connected' AND visibility='private'")
            .bind(owner).bind(project).fetch_optional(&self.pool).await?
            .ok_or_else(|| ApiError::forbidden("project_cache_project_unavailable"))?;
        Ok((row, metadata))
    }
    pub async fn private_project_allocation_source(
        &self,
        id: &str,
        proof: &str,
        now: i64,
    ) -> Result<Value, ApiError> {
        self.private_project_allocation_source_at(id, proof, now, "https://api.github.com")
            .await
    }
    pub(crate) async fn private_project_allocation_source_at(
        &self,
        id: &str,
        proof: &str,
        now: i64,
        origin: &str,
    ) -> Result<Value, ApiError> {
        let (before, project) = self.private_project_source_binding(id, proof, now).await?;
        let owner = required(&before, "account_id")?;
        let config = &before["request_config"];
        let actor = required(config, "rustActorId")?;
        let user = config["rustGithubUserId"]
            .as_i64()
            .filter(|id| *id > 0)
            .ok_or_else(|| ApiError::forbidden("github_task_credential_binding_required"))?;
        let repository = required(&project, "repository_identity")?;
        let name = repository
            .strip_prefix("github.com/")
            .ok_or_else(|| ApiError::conflict("project_cache_repository_invalid"))?;
        let authority = self
            .scope_authority
            .get()
            .and_then(|a| a.github.as_ref())
            .ok_or_else(|| ApiError::unavailable("project_cache_source_authority_unavailable"))?;
        if authority.connected_user_id(actor).await? != user {
            return Err(ApiError::forbidden("github_account_changed"));
        }
        let url = format!("https://{repository}.git");
        let authorization = authority.authorize_repository(owner, actor, &url).await?;
        let credential = authority.scoped_git(owner, actor, name).await?;
        if credential.github_user_id != user {
            return Err(ApiError::forbidden("github_account_changed"));
        }
        let recipe = serde_json::from_value(config["environmentRecipe"].clone())
            .map_err(|_| ApiError::conflict("environment_recipe_invalid"))?;
        let reference = config["rustResolvedSource"]["commit"]
            .as_str()
            .or_else(|| before["base_ref"].as_str())
            .ok_or_else(|| ApiError::conflict("project_cache_repository_invalid"))?;
        let source = crate::project_caches::ref_resolution::resolve_authenticated_at(
            origin,
            repository,
            reference,
            recipe,
            Some(credential.token.as_str()),
        )
        .await?;
        if let Some(expected) = config["stagingProjectCachePilotSha256"].as_str() {
            let pilot =
                crate::project_caches::pilot::current(chrono::Utc::now().timestamp_millis())?
                    .ok_or_else(|| ApiError::forbidden("staging_project_cache_pilot_fenced"))?;
            if pilot.pilot.fingerprint()? != expected {
                return Err(ApiError::forbidden("staging_project_cache_pilot_fenced"));
            }
            pilot
                .pilot
                .source_allowed(owner, required(&before, "project_id")?, &source)?;
        }
        let (after, current) = self
            .private_project_source_binding(id, proof, chrono::Utc::now().timestamp_millis())
            .await?;
        if after["account_id"] != before["account_id"]
            || after["project_id"] != before["project_id"]
            || after["base_ref"] != before["base_ref"]
            || after["request_config"] != before["request_config"]
            || current["repository_identity"] != project["repository_identity"]
        {
            return Err(ApiError::forbidden("project_cache_allocation_fenced"));
        }
        if authority.connected_user_id(actor).await? != user {
            return Err(ApiError::forbidden("github_account_changed"));
        }
        // Recheck the exact prefetch scope/user epochs, not a newly minted proof.
        let mut tx = self.pool.begin().await?;
        authority
            .revalidate_repository(&mut tx, &authorization, &url)
            .await?;
        tx.commit().await?;
        serde_json::to_value(source)
            .map_err(|_| ApiError::unavailable("project_cache_source_response_invalid"))
    }
}
