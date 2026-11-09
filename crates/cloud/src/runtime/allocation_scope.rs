//! Workers consult the API-owned credential/scope authority before provider
//! effects; signing/OAuth secrets remain API-only. No SQL lock crosses HTTP.
use super::*;
impl RuntimeControl {
    #[tracing::instrument(name="cloud_allocation_stage", skip_all, fields(workspace=%row["workspace_id"], operation="allocation_scope"))]
    pub(crate) async fn allocation_scope_access(
        &self,
        row: &Value,
        operation: &Value,
        now: i64,
    ) -> Result<(), ApiError> {
        self.candidate_row_allowed(row, now).await?;
        if !row["account_id"]
            .as_str()
            .is_some_and(|o| o.starts_with("organization:"))
        {
            return Ok(());
        }
        let id = required(row, "workspace_id")?;
        if self.scope_authority_configured() {
            self.scope_access(id, None, true).await?;
            return Ok(());
        }
        let proof = self.signer.allocation_scope_proof(
            id,
            required(operation, "operation_id")?,
            operation["generation"]
                .as_i64()
                .ok_or_else(|| ApiError::conflict("cloud_allocation_fenced"))?,
            now,
        )?;
        let mut url = reqwest::Url::parse(&self.signer.issuer())
            .map_err(|_| ApiError::unavailable("allocation_scope_unavailable"))?;
        url.path_segments_mut()
            .map_err(|_| ApiError::unavailable("allocation_scope_unavailable"))?
            .clear()
            .extend(["v1", "cloud", "workspaces", id, "allocation-scope"]);
        let response = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(5))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| ApiError::unavailable("allocation_scope_unavailable"))?
            .post(url)
            .bearer_auth(proof)
            .send()
            .await
            .map_err(|_| ApiError::unavailable("allocation_scope_unavailable"))?;
        if response.status() != reqwest::StatusCode::NO_CONTENT {
            return Err(ApiError::forbidden("allocation_scope_denied"));
        }
        Ok(())
    }
    pub async fn authorize_allocation_scope(
        &self,
        id: &str,
        proof: &str,
        now: i64,
    ) -> Result<(), ApiError> {
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
            || !super::scope::native_organization_task(&row)
        {
            return Err(ApiError::forbidden("allocation_scope_denied"));
        }
        let exists:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM zuse_rust_allocations WHERE operation_id=$1 AND workspace_id=$2 AND generation=$3)").bind(claims["operationId"].as_str()).bind(id).bind(claims["generation"].as_i64()).fetch_one(&self.pool).await?;
        if !exists {
            return Err(ApiError::forbidden("allocation_scope_denied"));
        }
        self.scope_access(id, None, true).await?;
        Ok(())
    }
}
