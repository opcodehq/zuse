//! Admin policy and roster reconciliation are serialized with seat admissions.
//! A stale or failed roster never removes memberships.
use super::*;
impl Joining {
    pub async fn set_policy(
        &self,
        access: &Access,
        installation: i64,
        enabled: bool,
    ) -> Result<(), ApiError> {
        let org = admin(access)?;
        self.rollout(org)?;
        let mut tx = self.lock(org).await?;
        let metadata:Value=sqlx::query_scalar("SELECT metadata FROM zuse_rust_github_installations WHERE owner_id=$1 AND installation_id=$2 AND state<>'removed'").bind(&access.owner_id).bind(installation).fetch_optional(&mut *tx).await?.ok_or_else(||ApiError::forbidden("github_installation_not_connected"))?;
        let github_org_id = metadata["account"]["id"]
            .as_i64()
            .filter(|id| *id > 0)
            .ok_or_else(unavailable)?;
        let login = metadata["account"]["login"]
            .as_str()
            .filter(|login| valid_component(login))
            .ok_or_else(unavailable)?
            .to_owned();
        if metadata["account"]["type"] != "Organization" {
            return Err(ApiError::forbidden("github_installation_unavailable"));
        }
        let policy = Policy {
            organization_id: org.into(),
            installation_id: installation,
            github_org_id,
            login,
            enabled,
            revision: uuid::Uuid::new_v4().to_string(),
        };
        // Enabling requires a complete fresh roster and Members permission. A cached
        // installation record alone is insufficient permission for automatic joins.
        if enabled {
            self.roster.members(&policy).await?;
        }
        let current:Option<Value>=sqlx::query_scalar("SELECT metadata FROM zuse_rust_github_installations WHERE owner_id=$1 AND installation_id=$2 AND state<>'removed' AND ($3=false OR state='active') FOR SHARE").bind(&access.owner_id).bind(installation).bind(enabled).fetch_optional(&mut *tx).await?;
        if current.as_ref().is_none_or(|v| {
            v["account"]["id"] != github_org_id || v["account"]["type"] != "Organization"
        }) {
            return Err(ApiError::conflict("github_installation_changed"));
        }

        sqlx::query("INSERT INTO api_github_join_policies(organization_id,installation_id,github_org_id,data) VALUES($1,$2,$3,$4) ON CONFLICT(organization_id,installation_id) DO UPDATE SET github_org_id=EXCLUDED.github_org_id,data=EXCLUDED.data").bind(org).bind(installation).bind(github_org_id).bind(json!(policy)).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(())
    }
    pub async fn settings(&self, access: &Access) -> Result<Value, ApiError> {
        let org = admin(access)?;
        self.rollout(org)?;
        let policies:Vec<Value>=sqlx::query_scalar("SELECT data FROM api_github_join_policies WHERE organization_id=$1 ORDER BY installation_id").bind(org).fetch_all(&self.pool).await?;
        let blocked:Vec<Value>=sqlx::query_scalar("SELECT data FROM api_github_enrollments WHERE organization_id=$1 AND data->>'blocked'='true' ORDER BY account_id").bind(org).fetch_all(&self.pool).await?;
        Ok(json!({"policies":policies,"blockedMembers":blocked}))
    }
    /// Called by confirmed installation/membership events and explicit refresh.
    /// Policy revision is checked again after the network roster read and before
    /// any WorkOS mutation; webhook payload contents never authorize removals.
    pub async fn reconcile_installation(
        &self,
        installation: i64,
        now: i64,
    ) -> Result<(), ApiError> {
        let policies: Vec<Value> = sqlx::query_scalar(
            "SELECT data FROM api_github_join_policies WHERE installation_id=$1",
        )
        .bind(installation)
        .fetch_all(&self.pool)
        .await?;
        for value in policies {
            let observed: Policy = serde_json::from_value(value)
                .map_err(|_| ApiError::unavailable("github_policy_invalid"))?;
            if !self.organizations.contains(&observed.organization_id) {
                continue;
            }
            let roster = self.roster.members(&observed).await?;
            let mut tx = self.lock(&observed.organization_id).await?;
            let current = Self::policy(&mut tx, &observed.organization_id, installation).await?;
            if current != observed {
                return Err(ApiError::conflict("github_policy_changed"));
            }
            let enrolled:Vec<Value>=sqlx::query_scalar("SELECT data FROM api_github_enrollments WHERE organization_id=$1 AND installation_id=$2 FOR UPDATE").bind(&observed.organization_id).bind(installation).fetch_all(&mut *tx).await?;
            let memberships = self.directory.members(&observed.organization_id).await?;
            for value in enrolled {
                let e: Enrollment = serde_json::from_value(value)
                    .map_err(|_| ApiError::unavailable("github_enrollment_invalid"))?;
                if roster.contains(&e.github_user_id) || e.blocked {
                    continue;
                }
                if let Some(member) = memberships.iter().find(|m| {
                    m.user_id == e.account_id
                        && m.organization_id == observed.organization_id
                        && Some(m.id.as_str()) == e.member_id.as_deref()
                        && m.status == "active"
                }) {
                    self.directory.deactivate(&member.id).await?;
                }
            }
            tx.commit().await?;
            if observed.enabled {
                let identities:Vec<(String,i64)>=sqlx::query_as("SELECT i.account_id,i.github_user_id FROM api_github_identities i JOIN api_github_org_members r USING(github_user_id) WHERE r.installation_id=$1 ORDER BY i.account_id").bind(installation).fetch_all(&self.pool).await?;
                for (actor, id) in identities {
                    match self
                        .join(&actor, id, &observed.organization_id, installation, now)
                        .await
                    {
                        Ok(_) => {}
                        Err(e) if e.code == "organization_member_limit_reached" => break,
                        Err(e)
                            if matches!(
                                e.code,
                                "organization_access_denied" | "organization_workspaces_disabled"
                            ) => {}
                        Err(e) => return Err(e),
                    }
                }
            }
        }
        Ok(())
    }
    /// An explicit admin removal creates a durable block so future membership
    /// events do not automatically re-admit the removed member.
    pub async fn block(&self, access: &Access, actor: &str) -> Result<(), ApiError> {
        let org = admin(access)?;
        self.rollout(org)?;
        if actor == access.actor.account_id {
            return Err(ApiError::bad_request("organization_cannot_remove_self"));
        }
        let mut tx = self.lock(org).await?;
        let mut e = Self::enrollment(&mut tx, org, actor)
            .await?
            .ok_or_else(|| ApiError::not_found("github_enrollment_not_found"))?;
        let members = self.directory.members(org).await?;
        for m in members
            .iter()
            .filter(|m| m.organization_id == org && m.user_id == actor && m.status == "active")
        {
            if m.role.slug == "admin"
                && members
                    .iter()
                    .filter(|other| {
                        other.organization_id == org
                            && other.status == "active"
                            && other.role.slug == "admin"
                    })
                    .count()
                    <= 1
            {
                return Err(ApiError::conflict("organization_last_admin"));
            }
            self.directory.deactivate(&m.id).await?;
        }
        e.blocked = true;
        e.reserved_until = 0;
        e.revision = uuid::Uuid::new_v4().to_string();
        Self::save(&mut tx, &e).await?;
        tx.commit().await?;
        Ok(())
    }
}
