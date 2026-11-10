//! Pure candidate checks for a row returned by the same transaction's0023 lock.
use super::*;
impl StagingBasePin {
    pub(crate) fn candidate_from_locked_record(
        &self,
        row: &Value,
        now: i64,
    ) -> Result<ManagedBaseCandidate, ApiError> {
        if row["state"] != "preparing"
            || !row["runtime_manifest"].is_null()
            || row["candidate_manifest"].is_null()
        {
            return Err(ApiError::conflict(
                "staging_managed_base_candidate_unavailable",
            ));
        }
        let candidate: ManagedBaseCandidate =
            serde_json::from_value(row["candidate_manifest"].clone()).map_err(|_| invalid())?;
        candidate.validate()?;
        self.matches(&candidate, now)?;
        if row["provider"] != self.provider.as_str()
            || row["release_id"] != self.release_id
            || row["artifact_id"] != candidate.artifact_id
            || row["architecture"] != candidate.architecture
            || row["capabilities"] != json!(candidate.capabilities)
        {
            return Err(invalid());
        }
        Ok(candidate)
    }
    pub(crate) fn source_from_locked_record(
        &self,
        row: &Value,
        owner: &str,
        provider: &str,
        agent: &str,
        model: &str,
        size: &str,
        organization: bool,
        now: i64,
    ) -> Result<Value, ApiError> {
        if !self.applies(owner, provider)
            || organization != owner.starts_with("organization:")
            || organization && self.actor_id.is_none()
        {
            return Err(ApiError::forbidden(
                "staging_candidate_actor_scope_required",
            ));
        }
        let candidate = self.candidate_from_locked_record(row, now)?;
        let agent = if agent == "claude-code" {
            "claude"
        } else {
            agent
        };
        if !candidate
            .harnesses
            .iter()
            .any(|h| h.agent.as_str() == agent && h.catalog.resolve_model(model).is_some())
            || !candidate.capabilities.sizes.iter().any(|s| s.id == size)
        {
            return Err(ApiError::conflict(
                "staging_candidate_unavailable_for_agent",
            ));
        }
        Ok(
            json!({"build_id":null,"snapshot_id":candidate.artifact_id,"settings":candidate.settings(self)}),
        )
    }
}

impl StagingBasePin {
    pub(crate) fn validate_row_observation(
        &self,
        row: &Value,
        observed: &Value,
        now: i64,
    ) -> Result<(), ApiError> {
        let effective = self.pin_for_row(row)?;
        if observed["schema"] != true {
            return Err(ApiError::conflict(
                "staging_managed_base_candidate_schema_required",
            ));
        }
        if observed["record"].is_null() {
            return Err(ApiError::conflict(
                "staging_managed_base_candidate_unavailable",
            ));
        }
        effective.candidate_from_locked_record(&observed["record"], now)?;
        if effective.candidate_sha256 != self.candidate_sha256 {
            self.validate_inventory_observation(&observed["inventory"])?;
        }
        Ok(())
    }
    fn validate_inventory_observation(&self, inventory: &Value) -> Result<(), ApiError> {
        let count = inventory["count"].as_i64().unwrap_or(0);
        if count < 1
            || self.computer_limit().is_some_and(|limit| count > limit)
            || inventory["owns"] != true
        {
            return Err(ApiError::conflict("staging_candidate_computer_limit"));
        }
        Ok(())
    }
    pub(crate) fn validate_capacity_observation(
        &self,
        row: &Value,
        observed: &Value,
        now: i64,
    ) -> Result<bool, ApiError> {
        self.validate_row_observation(row, observed, now)?;
        let effective = self.pin_for_row(row)?;
        if !effective.allow_staging_compute {
            return Ok(false);
        }
        self.validate_inventory_observation(&observed["inventory"])?;
        Ok(true)
    }
}
