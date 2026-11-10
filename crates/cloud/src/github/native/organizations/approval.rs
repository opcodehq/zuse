//! Account consent and installation approval are independent. Save a verified
//! identity now so organization administrators can approve the App later,
//! without retaining a browser transaction or repeating OAuth.
use super::*;
impl NativeGithub {
    pub async fn confirm_identity_browser(
        &self,
        state: &str,
        browser: Option<&str>,
    ) -> Result<BrowserTransaction, ApiError> {
        let row = self.browser_row(state, browser).await?;
        let id: String = row.try_get("operation_id")?;
        if row.try_get::<String, _>("phase")? != "choosing" {
            return Err(ApiError::conflict("github_operation_unavailable"));
        }
        let sealed: String = row.try_get("sealed_pending")?;
        let mut pending: Pending = self.open(&format!("github-pending:{id}"), &sealed)?;
        if pending.browser_installation.is_some_and(|id| id != 0) {
            return Err(ApiError::conflict("github_browser_choice_changed"));
        }
        pending.browser_installation = Some(0);
        let next = self.seal(&format!("github-pending:{id}"), &pending)?;
        let changed=sqlx::query("UPDATE zuse_rust_github_operations SET sealed_pending=$3 WHERE operation_id=$1 AND phase='choosing' AND sealed_pending=$2 AND expires_at>$4").bind(&id).bind(&sealed).bind(next).bind(now()).execute(&self.pool).await?;
        if changed.rows_affected() != 1 {
            return Err(ApiError::conflict("github_operation_unavailable"));
        }
        Ok(BrowserTransaction {
            actor: row.try_get("actor_id")?,
            owner: row.try_get("owner_id")?,
            operation_id: id,
        })
    }
    pub async fn finalize_identity(&self, access: &Access, id: &str) -> Result<(), ApiError> {
        self.require(access)?;
        let row=sqlx::query("SELECT actor_epoch,scope_epoch,sealed_pending FROM zuse_rust_github_operations WHERE operation_id=$1 AND actor_id=$2 AND owner_id=$3 AND phase='choosing' AND expires_at>$4").bind(id).bind(&access.actor.account_id).bind(&access.owner_id).bind(now()).fetch_optional(&self.pool).await?.ok_or_else(||ApiError::conflict("github_operation_unavailable"))?;
        let pending: Pending = self.open(
            &format!("github-pending:{id}"),
            &row.try_get::<String, _>("sealed_pending")?,
        )?;
        if pending.browser_installation != Some(0) {
            return Err(ApiError::forbidden("github_browser_confirmation_required"));
        }
        let verified = self.get("/user", &pending.credentials.access_token).await?;
        if verified["id"] != pending.profile["id"] {
            return Err(ApiError::forbidden("github_identity_changed"));
        }
        let enrollment = self.verify_pending_enrollment(access, &pending).await?;
        let epoch: i64 = row.try_get("actor_epoch")?;
        let scope_epoch: i64 = row.try_get("scope_epoch")?;
        let sealed = self.seal(
            &format!("github-user:{}:{}", access.actor.account_id, epoch + 1),
            &pending.credentials,
        )?;
        let mut tx = self.pool.begin().await?;
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
            .bind(format!("github-user:{}", access.actor.account_id))
            .execute(&mut *tx)
            .await?;
        self.lock_scope_epoch(&mut tx, &access.owner_id, scope_epoch)
            .await?;
        self.lock_enrollment(&mut tx, access, &enrollment).await?;
        let claimed=sqlx::query("UPDATE zuse_rust_github_operations o SET phase='complete',sealed_pending=NULL WHERE operation_id=$1 AND phase='choosing' AND expires_at>$2 AND actor_epoch=$3 AND scope_epoch=$4 AND EXISTS(SELECT 1 FROM zuse_rust_github_users u WHERE u.actor_id=o.actor_id AND u.epoch=o.actor_epoch AND u.state<>'revoking') AND EXISTS(SELECT 1 FROM zuse_rust_github_scopes s WHERE s.owner_id=o.owner_id AND s.epoch=o.scope_epoch)").bind(id).bind(now()).bind(epoch).bind(scope_epoch).execute(&mut *tx).await?;
        if claimed.rows_affected() != 1 {
            return Err(ApiError::conflict("github_operation_unavailable"));
        }
        let changed=sqlx::query("UPDATE zuse_rust_github_users SET github_user_id=$2,profile=$3,sealed_credentials=$4,state='connected',refresh_uncertain=false,epoch=epoch+1,version=version+1,updated_at=$5 WHERE actor_id=$1 AND epoch=$6 AND state<>'revoking' AND (state='disconnected' OR github_user_id IS NULL OR github_user_id=$2)").bind(&access.actor.account_id).bind(pending.profile["id"].as_i64().ok_or_else(unavailable)?).bind(&pending.profile).bind(sealed).bind(now()).bind(epoch).execute(&mut *tx).await?;
        if changed.rows_affected() != 1 {
            return Err(ApiError::conflict("github_identity_changed"));
        }
        self.commit_join_identity(&mut tx, access, &pending, id, &Value::Null)
            .await?;
        tx.commit().await?;
        Ok(())
    }
    pub(in crate::github::native) async fn reusable_identity(
        &self,
        access: &Access,
    ) -> Result<Option<(Pending, i64, i64)>, ApiError> {
        if self.connection_state(access).await? != "connected" {
            return Ok(None);
        }
        let credentials = match self.credentials(&access.actor.account_id).await {
            Ok(c) => c,
            Err(e)
                if matches!(
                    e.code,
                    "github_reconnect_required" | "github_refresh_uncertain"
                ) =>
            {
                return Ok(None);
            }
            Err(e) => return Err(e),
        };
        let (epoch, version, profile) = self
            .credential_binding(&access.actor.account_id, &credentials.access_token)
            .await?;
        let user = self.get("/user", &credentials.access_token).await?;
        if user["id"] != profile["id"] {
            return Err(ApiError::forbidden("github_identity_changed"));
        }
        Ok(Some((
            Pending {
                credentials,
                profile,
                browser_installation: None,
            },
            epoch,
            version,
        )))
    }
}
