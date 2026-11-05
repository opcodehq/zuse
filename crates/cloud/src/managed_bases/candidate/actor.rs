//! Optional staging-only actor binding for an organization canary.
//! Personal pins retain their previous wire shape and single-owner cohort.
use super::*;
impl ManagedBaseCandidate {
    pub async fn stage_for_actor_url(
        &self,
        url: &str,
        owner: &str,
        actor: &str,
        ttl_seconds: i64,
        allow_staging_compute: bool,
        now: i64,
    ) -> Result<StagingBasePin, ApiError> {
        if std::env::var("ZUSE_DEPLOYMENT_ENVIRONMENT").as_deref() != Ok("staging")
            || !(60..=86400).contains(&ttl_seconds)
        {
            return Err(ApiError::forbidden("staging_candidate_deployment_required"));
        }
        let pin = self.pin_for_actor(
            owner,
            actor,
            now + ttl_seconds * 1000,
            allow_staging_compute,
        )?;
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .connect(url)
            .await
            .map_err(|_| ApiError::unavailable("managed_base_database_unavailable"))?;
        self.stage(&pool, &pin, now).await?;
        Ok(pin)
    }
    pub fn pin_for_actor(
        &self,
        owner: &str,
        actor: &str,
        expires_at: i64,
        allow_staging_compute: bool,
    ) -> Result<StagingBasePin, ApiError> {
        // Pin construction is pure; no identity or credential is copied between owners.
        let mut pin = self.pin(actor, expires_at, allow_staging_compute)?;
        pin.owner_id = owner.to_owned();
        pin.actor_id = Some(actor.to_owned());
        pin.validate()?;
        if owner.starts_with("organization:")
            && self.manifest.rust_organization_execution_version != Some(1)
        {
            return Err(ApiError::conflict(
                "managed_base_organization_runtime_required",
            ));
        }
        Ok(pin)
    }
}
impl StagingBasePin {
    pub(super) fn validate_scope(&self) -> Result<(), ApiError> {
        let identity = |id: &str| {
            !id.is_empty()
                && id.len() <= 160
                && id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
        };
        if let Some(org) = self.owner_id.strip_prefix("organization:") {
            if org.is_empty()
                || org.len() > 128
                || !org
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
                || self
                    .actor_id
                    .as_deref()
                    .is_none_or(|actor| !identity(actor) || actor.starts_with("organization:"))
            {
                return Err(invalid());
            }
        } else if !identity(&self.owner_id)
            || self
                .actor_id
                .as_deref()
                .is_some_and(|actor| actor != self.owner_id)
        {
            return Err(invalid());
        }
        Ok(())
    }
    pub(crate) fn require_actor(&self, actor: &str) -> Result<(), ApiError> {
        self.validate_scope()?;
        let expected = self.actor_id.as_deref().unwrap_or(&self.owner_id);
        if actor != expected {
            return Err(ApiError::forbidden("staging_candidate_actor_not_selected"));
        }
        Ok(())
    }
    pub(super) fn auth_cohort_matches(&self, auth: &[String]) -> bool {
        if self.owner_id.starts_with("organization:") {
            let Some(actor) = self.actor_id.as_ref() else {
                return false;
            };
            let expected =
                std::collections::BTreeSet::from([self.owner_id.as_str(), actor.as_str()]);
            auth.len() == 2
                && auth
                    .iter()
                    .map(String::as_str)
                    .collect::<std::collections::BTreeSet<_>>()
                    == expected
        } else {
            auth == [self.owner_id.clone()]
        }
    }
    pub(super) fn require_row_actor(&self, row: &Value) -> Result<(), ApiError> {
        if self.owner_id.starts_with("organization:") {
            if !crate::runtime::scope::native_organization_task(row)
                || row["request_config"]["rustActorId"].as_str() != self.actor_id.as_deref()
            {
                return Err(ApiError::forbidden(
                    "staging_managed_base_candidate_actor_fenced",
                ));
            }
        }
        Ok(())
    }
}
impl crate::store::Store {
    pub async fn computer_capabilities_for_actor(
        &self,
        owner: &str,
        actor: &str,
        now: i64,
    ) -> Result<zuse_protocol::cloud_computers::CloudCapabilities, ApiError> {
        let mut caps = self.computer_capabilities(owner, now).await?;
        if self
            .staging_candidate
            .as_ref()
            .is_some_and(|pin| pin.owner_id == owner && pin.require_actor(actor).is_err())
        {
            caps.can_allocate = false;
            caps.allocation_blockers
                .push("staging_candidate_actor_not_selected".into());
        }
        Ok(caps)
    }
}
#[cfg(test)]
#[path = "actor_tests.rs"]
mod tests;
