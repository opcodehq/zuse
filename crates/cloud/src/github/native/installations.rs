//! Installation links are scoped separately from the actor's OAuth identity.
use super::*;
impl NativeGithub {
    async fn writable_installation_subset(
        &self,
        metadata: &Value,
        token: &str,
    ) -> Result<Option<Value>, ApiError> {
        let id = metadata["id"]
            .as_i64()
            .filter(|id| *id > 0)
            .ok_or_else(unavailable)?;
        let names: BTreeSet<String> = self
            .repositories(id, token)
            .await?
            .iter()
            .filter(|repo| repository_allowed(metadata, None, repo))
            .filter_map(|repo| repo["full_name"].as_str().map(str::to_owned))
            .collect();
        if names.is_empty() {
            return Err(ApiError::forbidden("github_repository_access_denied"));
        }
        Ok(Some(json!(names)))
    }
    pub(super) async fn installation_access(
        &self,
        access: &Access,
        metadata: &Value,
        profile: &Value,
        token: &str,
    ) -> Result<Option<Value>, ApiError> {
        let personal = matches!(access.scope, crate::access::Scope::Personal)
            && access.owner_id == access.actor.account_id;
        match metadata["account"]["type"].as_str() {
            Some("User") if metadata["account"]["id"] == profile["id"] => Ok(None),
            // Outside collaborators receive only their own currently writable
            // repository subset. This never grants organization enrollment.
            Some("User") if personal => self.writable_installation_subset(metadata, token).await,
            Some("Organization") => {
                let login = metadata["account"]["login"]
                    .as_str()
                    .filter(|s| valid_component(s))
                    .ok_or_else(unavailable)?;
                let (status, member) = self
                    .response(
                        self.http
                            .get(self.api(&format!("/user/memberships/orgs/{login}")))
                            .bearer_auth(token),
                    )
                    .await?;
                if status == 403 {
                    return Err(ApiError::conflict("github_organization_approval_required"));
                }
                if status == 404 && personal {
                    return self.writable_installation_subset(metadata, token).await;
                }
                if status == 401 {
                    return Err(reconnect());
                }
                if status != 200 && status != 404 {
                    return Err(unavailable());
                }
                if status != 200 || member["state"] != "active" {
                    return Err(ApiError::forbidden(
                        "github_organization_membership_required",
                    ));
                }
                match member["role"].as_str() {
                    Some("admin") => Ok(None),
                    Some("member") => self.writable_installation_subset(metadata, token).await,
                    _ => Err(ApiError::forbidden(
                        "github_organization_membership_required",
                    )),
                }
            }
            _ => Err(ApiError::forbidden("github_installation_identity_mismatch")),
        }
    }
    pub(super) async fn lock_scope_epoch(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        owner: &str,
        expected: i64,
    ) -> Result<(), ApiError> {
        let epoch: Option<i64> = sqlx::query_scalar(
            "SELECT epoch FROM zuse_rust_github_scopes WHERE owner_id=$1 FOR UPDATE",
        )
        .bind(owner)
        .fetch_optional(&mut **tx)
        .await?;
        if epoch != Some(expected) {
            return Err(ApiError::conflict("github_operation_unavailable"));
        }
        Ok(())
    }
    /// Remove this workspace's installation authorization, not another scope's
    /// account identity. Explicit account disconnect remains a global revocation.
    pub async fn remove_installation(&self, access: &Access, id: i64) -> Result<Value, ApiError> {
        self.require(access)?;
        if !can_manage(access) {
            return Err(ApiError::forbidden("github_installation_admin_required"));
        }
        if id <= 0 {
            return Err(ApiError::bad_request("invalid_github_installation"));
        }
        let mut tx = self.pool.begin().await?;
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
            .bind(format!("github-user:{}", access.actor.account_id))
            .execute(&mut *tx)
            .await?;
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM zuse_rust_github_scopes WHERE owner_id=$1)",
        )
        .bind(&access.owner_id)
        .fetch_one(&mut *tx)
        .await?;
        if !exists {
            return Err(ApiError::not_found("github_installation_not_found"));
        }
        sqlx::query("SELECT epoch FROM zuse_rust_github_scopes WHERE owner_id=$1 FOR UPDATE")
            .bind(&access.owner_id)
            .execute(&mut *tx)
            .await?;
        let state:Option<String>=sqlx::query_scalar("SELECT state FROM zuse_rust_github_installations WHERE owner_id=$1 AND installation_id=$2").bind(&access.owner_id).bind(id).fetch_optional(&mut *tx).await?;
        let Some(state) = state else {
            return Err(ApiError::not_found("github_installation_not_found"));
        };
        let changed = state != "removed";
        if changed {
            sqlx::query("UPDATE zuse_rust_github_installations SET state='removed',observation_id=NULL,revision=revision+1,updated_at=$3 WHERE owner_id=$1 AND installation_id=$2").bind(&access.owner_id).bind(id).bind(now()).execute(&mut *tx).await?;
            sqlx::query("UPDATE zuse_rust_github_scopes SET epoch=epoch+1 WHERE owner_id=$1")
                .bind(&access.owner_id)
                .execute(&mut *tx)
                .await?;
            sqlx::query("UPDATE zuse_rust_github_operations SET phase='cancelled',sealed_pending=NULL WHERE owner_id=$1 AND phase NOT IN ('complete','cancelled','failed')").bind(&access.owner_id).execute(&mut *tx).await?;
            self.commit_membership_jobs(
                &mut tx,
                "installation",
                &json!({"installation":{"id":id},"action":"deleted"}),
            )
            .await?;
        }
        tx.commit().await?;
        if changed {
            self.membership_epoch
                .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        }
        Ok(
            json!({"installationId":id,"removed":true,"changed":changed,"accountDisconnected":false}),
        )
    }
}
