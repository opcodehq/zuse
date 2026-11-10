//! Existing task exceptions inherit the current staging owner, actor and expiry.
//! They do not participate in source selection for a new computer.
use super::*;

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RetainedWorkspaceBinding {
    pub workspace_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<ProviderId>,
    pub release_id: String,
    pub artifact_id: String,
    pub harness_release: String,
    pub manifest_sha256: String,
    pub candidate_sha256: String,
}

impl StagingBasePin {
    pub(super) fn validate_retained(&self) -> Result<(), ApiError> {
        // Preserve admitted computers under the existing staging capacity policy.
        // Even an unlimited compute pilot keeps its configuration bounded.
        let retained_limit = if self.unlimited_computers {
            128
        } else {
            usize::from(self.max_computers.unwrap_or(2))
        };
        if self.retained_workspaces.len() > retained_limit
            || !self.retained_workspaces.is_empty() && !self.allow_staging_compute
        {
            return Err(invalid());
        }
        let mut ids = std::collections::BTreeSet::new();
        for binding in &self.retained_workspaces {
            if !identifier(&binding.workspace_id)
                || !ids.insert(&binding.workspace_id)
                || !identifier(&binding.release_id)
                || !identifier(&binding.artifact_id)
                || !sha256(&binding.harness_release)
                || !sha256(&binding.manifest_sha256)
                || !sha256(&binding.candidate_sha256)
                || binding.candidate_sha256 == self.candidate_sha256
            {
                return Err(invalid());
            }
        }
        Ok(())
    }
    pub(super) async fn retained_row_inventory(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        effective: &Self,
        row: &Value,
    ) -> Result<(), ApiError> {
        if effective.candidate_sha256 == self.candidate_sha256 {
            return Ok(());
        }
        let (count, owns): (i64, Option<bool>) = sqlx::query_as("SELECT count(*),bool_or(workspace_id=$2 AND provider=$3 AND request_config->>'managedReleaseId'=$4 AND request_config->>'managedCandidateSha256'=$5 AND request_config->>'managedCandidateArtifactId'=$6) FROM api_cloud_workspaces WHERE account_id=$1 AND state<>'deleted' AND request_config ? 'managedCandidateSha256'")
            .bind(&self.owner_id).bind(row["workspace_id"].as_str().unwrap_or(""))
            .bind(effective.provider.as_str()).bind(&effective.release_id)
            .bind(&effective.candidate_sha256).bind(&effective.artifact_id)
            .fetch_one(&mut **tx).await?;
        if count < 1
            || self.computer_limit().is_some_and(|limit| count > limit)
            || owns != Some(true)
        {
            return Err(ApiError::conflict("staging_candidate_computer_limit"));
        }
        Ok(())
    }
    pub(crate) fn pin_for_row(&self, row: &Value) -> Result<Self, ApiError> {
        self.validate()?;
        self.require_row_actor(row)?;
        if row["account_id"].as_str() != Some(self.owner_id.as_str()) {
            return Err(ApiError::forbidden("staging_managed_base_candidate_fenced"));
        }
        if self.row_binding(row).is_ok() {
            return Ok(self.clone());
        }
        let binding = self
            .retained_workspaces
            .iter()
            .find(|binding| row["workspace_id"].as_str() == Some(binding.workspace_id.as_str()))
            .ok_or_else(|| ApiError::forbidden("staging_managed_base_candidate_fenced"))?;
        let mut retained = self.clone();
        retained.provider = binding.provider.unwrap_or(self.provider);
        retained.release_id = binding.release_id.clone();
        retained.artifact_id = binding.artifact_id.clone();
        retained.harness_release = binding.harness_release.clone();
        retained.manifest_sha256 = binding.manifest_sha256.clone();
        retained.candidate_sha256 = binding.candidate_sha256.clone();
        retained.retained_workspaces.clear();
        retained.row_binding(row)?;
        Ok(retained)
    }
}
