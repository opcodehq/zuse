//! Existing GitHub-managed memberships remain bound to their original identity
//! and policy. Connecting an account is not an enrollment or seat operation.
use super::*;
impl NativeGithub {
    pub(super) async fn enrollment_snapshot(
        &self,
        access: &Access,
    ) -> Result<Option<Value>, ApiError> {
        let Some(org) = access.owner_id.strip_prefix("organization:") else {
            return Ok(None);
        };
        let schema:bool=sqlx::query_scalar("SELECT to_regclass('api_github_enrollments') IS NOT NULL AND to_regclass('api_github_join_policies') IS NOT NULL").fetch_one(&self.pool).await?;
        if !schema {
            return Ok(None);
        }
        sqlx::query_scalar("SELECT jsonb_build_object('enrollment',e.data,'installationId',e.installation_id,'policy',(SELECT p.data FROM api_github_join_policies p WHERE p.organization_id=e.organization_id AND p.installation_id=e.installation_id)) FROM api_github_enrollments e WHERE e.organization_id=$1 AND e.account_id=$2").bind(org).bind(&access.actor.account_id).fetch_optional(&self.pool).await.map_err(Into::into)
    }
    pub(super) async fn verify_pending_enrollment(
        &self,
        access: &Access,
        pending: &Pending,
    ) -> Result<Option<Value>, ApiError> {
        let snapshot = self.enrollment_snapshot(access).await?;
        let Some(record) = &snapshot else {
            return Ok(None);
        };
        let e = &record["enrollment"];
        let p = &record["policy"];
        if e["blocked"] != false
            || e["githubUserId"] != pending.profile["id"]
            || e["githubOrgId"] != p["githubOrgId"]
            || e["installationId"] != record["installationId"]
            || p["installationId"] != record["installationId"]
            || e["revision"].as_str().is_none()
            || p["revision"].as_str().is_none()
            || e["memberId"]
                .as_str()
                .is_some_and(|id| access.membership.as_ref().is_none_or(|m| m.id != id))
        {
            return Err(ApiError::forbidden("github_enrollment_identity_mismatch"));
        }
        let installation = record["installationId"]
            .as_i64()
            .filter(|id| *id > 0)
            .ok_or_else(|| ApiError::forbidden("organization_access_denied"))?;
        let metadata = self
            .observe(&access.owner_id, installation)
            .await?
            .ok_or_else(|| ApiError::forbidden("github_installation_unavailable"))?;
        if metadata["account"]["id"] != e["githubOrgId"]
            || metadata["account"]["type"] != "Organization"
        {
            return Err(ApiError::forbidden("github_enrollment_identity_mismatch"));
        }
        let login = metadata["account"]["login"]
            .as_str()
            .filter(|s| valid_component(s))
            .ok_or_else(unavailable)?;
        let membership = self
            .get(
                &format!("/user/memberships/orgs/{login}"),
                &pending.credentials.access_token,
            )
            .await?;
        if membership["state"] != "active"
            || !matches!(membership["role"].as_str(), Some("admin" | "member"))
        {
            return Err(ApiError::forbidden(
                "github_organization_membership_required",
            ));
        }
        Ok(snapshot)
    }
    pub(super) async fn lock_enrollment(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        access: &Access,
        snapshot: &Option<Value>,
    ) -> Result<(), ApiError> {
        let Some(org) = access.owner_id.strip_prefix("organization:") else {
            return Ok(());
        };
        if snapshot.is_none() {
            let schema: bool =
                sqlx::query_scalar("SELECT to_regclass('api_github_enrollments') IS NOT NULL")
                    .fetch_one(&mut **tx)
                    .await?;
            if schema {
                let appeared:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM api_github_enrollments WHERE organization_id=$1 AND account_id=$2)").bind(org).bind(&access.actor.account_id).fetch_one(&mut **tx).await?;
                if appeared {
                    return Err(ApiError::conflict("github_enrollment_changed"));
                }
            }
            return Ok(());
        }
        let current:Option<Value>=sqlx::query_scalar("SELECT jsonb_build_object('enrollment',e.data,'installationId',e.installation_id,'policy',p.data) FROM api_github_enrollments e JOIN api_github_join_policies p ON p.organization_id=e.organization_id AND p.installation_id=e.installation_id WHERE e.organization_id=$1 AND e.account_id=$2 FOR SHARE OF e,p").bind(org).bind(&access.actor.account_id).fetch_optional(&mut **tx).await?;
        if &current != snapshot {
            return Err(ApiError::conflict("github_enrollment_changed"));
        }
        Ok(())
    }
}
