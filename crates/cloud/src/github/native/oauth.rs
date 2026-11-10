use super::*;
#[derive(Deserialize)]
pub struct Callback {
    pub state: String,
    pub code: Option<String>,
    pub error: Option<String>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Selection {
    pub operation_id: String,
    pub installation_id: i64,
}
impl NativeGithub {
    pub async fn begin(&self, access: &Access) -> Result<Value, ApiError> {
        self.require(access)?;
        if self.connection_state(access).await? == "revoking" {
            self.disconnect(access).await?;
        }
        let reusable = self.reusable_identity(access).await?;
        let actor = &access.actor.account_id;
        let mut tx = self.pool.begin().await?;
        sqlx::query("INSERT INTO zuse_rust_github_users(actor_id,updated_at) VALUES($1,$2) ON CONFLICT DO NOTHING").bind(actor).bind(now()).execute(&mut *tx).await?;
        sqlx::query(
            "INSERT INTO zuse_rust_github_scopes(owner_id) VALUES($1) ON CONFLICT DO NOTHING",
        )
        .bind(&access.owner_id)
        .execute(&mut *tx)
        .await?;
        let epoch: i64 =
            sqlx::query_scalar("SELECT epoch FROM zuse_rust_github_users WHERE actor_id=$1")
                .bind(actor)
                .fetch_one(&mut *tx)
                .await?;
        let scope_epoch: i64 =
            sqlx::query_scalar("SELECT epoch FROM zuse_rust_github_scopes WHERE owner_id=$1")
                .bind(&access.owner_id)
                .fetch_one(&mut *tx)
                .await?;
        if let Some((_, expected_epoch, expected_version)) = &reusable {
            let valid:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM zuse_rust_github_users WHERE actor_id=$1 AND epoch=$2 AND version=$3 AND state='connected' AND NOT refresh_uncertain)").bind(actor).bind(expected_epoch).bind(expected_version).fetch_one(&mut *tx).await?;
            if !valid {
                return Err(ApiError::conflict("github_identity_changed"));
            }
        }
        let state = opaque();
        let id = uuid::Uuid::new_v4().to_string();
        let sealed = reusable
            .as_ref()
            .map(|(pending, _, _)| self.seal(&format!("github-pending:{id}"), pending))
            .transpose()?;
        sqlx::query("INSERT INTO zuse_rust_github_operations(operation_id,state_hash,actor_id,owner_id,actor_epoch,scope_epoch,phase,expires_at,created_at,sealed_pending) VALUES($1,$2,$3,$4,$5,$6,'created',$7,$8,$9)")
            .bind(&id).bind(hash(&state)).bind(actor).bind(&access.owner_id).bind(epoch).bind(scope_epoch).bind(now()+600_000).bind(now()).bind(sealed).execute(&mut *tx).await?;
        tx.commit().await?;
        let mut url = self
            .config
            .origin
            .join("v1/cloud/github/connect")
            .map_err(|_| unavailable())?;
        url.query_pairs_mut().append_pair("state", &state);
        Ok(json!({"url":url.as_str(),"operationId":id,"expiresAt":now()+600_000}))
    }
    /// The app bearer never enters the browser URL. Opening the opaque transaction
    /// binds it once to a secure browser cookie before GitHub receives OAuth state.
    pub async fn browser_start(
        &self,
        state: &str,
        browser: Option<&str>,
    ) -> Result<(String, String), ApiError> {
        if state.len() != 64 {
            return Err(ApiError::bad_request("invalid_github_state"));
        }
        let row=sqlx::query("SELECT browser_hash,phase,owner_id,sealed_pending FROM zuse_rust_github_operations WHERE state_hash=$1 AND expires_at>$2").bind(hash(state)).bind(now()).fetch_optional(&self.pool).await?.ok_or_else(||ApiError::bad_request("invalid_github_state"))?;
        let owner: String = row.try_get("owner_id")?;
        if !self.enabled(&owner) {
            return Err(ApiError::forbidden("github_native_disabled"));
        }
        let phase: String = row.try_get("phase")?;
        let previous: Option<String> = row.try_get("browser_hash")?;
        let browser = if let Some(expected) = previous {
            let actual = browser
                .filter(|b| b.len() == 64)
                .ok_or_else(|| ApiError::forbidden("github_browser_mismatch"))?;
            if hash(actual) != expected {
                return Err(ApiError::forbidden("github_browser_mismatch"));
            }
            actual.to_owned()
        } else {
            if phase != "created" {
                return Err(ApiError::conflict("github_operation_unavailable"));
            }
            opaque()
        };
        let reused = row
            .try_get::<Option<String>, _>("sealed_pending")?
            .is_some();
        if phase == "choosing" && reused {
            // Reopening the original private URL resumes the same browser-bound
            // selection. No new identity, callback exchange or expiry extension.
            let mut url = self
                .config
                .origin
                .join("v1/cloud/github/browser")
                .map_err(|_| unavailable())?;
            url.query_pairs_mut().append_pair("state", state);
            return Ok((url.to_string(), browser));
        }
        let updated=sqlx::query("UPDATE zuse_rust_github_operations SET browser_hash=$2,phase=$3 WHERE state_hash=$1 AND phase IN ('created','authorizing') AND (browser_hash IS NULL OR browser_hash=$2)")
            .bind(hash(state)).bind(hash(&browser)).bind(if reused{"choosing"}else{"authorizing"}).execute(&self.pool).await?;
        if updated.rows_affected() != 1 {
            return Err(ApiError::conflict("github_operation_unavailable"));
        }
        if reused {
            let mut url = self
                .config
                .origin
                .join("v1/cloud/github/browser")
                .map_err(|_| unavailable())?;
            url.query_pairs_mut().append_pair("state", state);
            return Ok((url.to_string(), browser));
        }
        let mut url = self
            .config
            .oauth_origin
            .join("login/oauth/authorize")
            .map_err(|_| unavailable())?;
        let callback = self
            .config
            .origin
            .join("v1/cloud/github/callback")
            .map_err(|_| unavailable())?;
        url.query_pairs_mut()
            .append_pair("client_id", &self.config.client_id)
            .append_pair("redirect_uri", callback.as_str())
            .append_pair("state", state);
        Ok((url.to_string(), browser))
    }
    pub async fn callback(&self, query: &Callback, browser: Option<&str>) -> Result<(), ApiError> {
        let browser = browser
            .filter(|s| s.len() == 64)
            .ok_or_else(|| ApiError::forbidden("github_browser_mismatch"))?;
        let row=sqlx::query("UPDATE zuse_rust_github_operations o SET phase='exchanging' WHERE state_hash=$1 AND browser_hash=$2 AND expires_at>$3 AND phase='authorizing' AND EXISTS(SELECT 1 FROM zuse_rust_github_users u WHERE u.actor_id=o.actor_id AND u.epoch=o.actor_epoch) AND EXISTS(SELECT 1 FROM zuse_rust_github_scopes s WHERE s.owner_id=o.owner_id AND s.epoch=o.scope_epoch) RETURNING operation_id,owner_id")
            .bind(hash(&query.state)).bind(hash(browser)).bind(now()).fetch_optional(&self.pool).await?.ok_or_else(||ApiError::conflict("github_operation_unavailable"))?;
        let id: String = row.try_get("operation_id")?;
        let owner: String = row.try_get("owner_id")?;
        if !self.enabled(&owner) {
            return Err(ApiError::forbidden("github_native_disabled"));
        }
        if query.error.is_some()
            || query
                .code
                .as_ref()
                .is_none_or(|c| c.is_empty() || c.len() > 1024)
        {
            self.fail_operation(&id).await?;
            return Err(ApiError::bad_request("github_authorization_cancelled"));
        }
        let result: Result<String, ApiError> = async {
            let credentials = self
                .exchange(&[("code", query.code.as_deref().unwrap())])
                .await?;
            let observed = self.get("/user", &credentials.access_token).await?;
            let user = public_user(&observed)?;
            let profile = json!({"id":observed["id"],"login":user.login,"name":user.name});
            self.seal(
                &format!("github-pending:{id}"),
                &Pending {
                    credentials,
                    profile,
                    browser_installation: None,
                },
            )
        }
        .await;
        let sealed = match result {
            Ok(value) => value,
            Err(error) => {
                self.fail_operation(&id).await?;
                return Err(error);
            }
        };
        let count=sqlx::query("UPDATE zuse_rust_github_operations o SET sealed_pending=$2,phase='choosing' WHERE operation_id=$1 AND phase='exchanging' AND expires_at>$3 AND EXISTS(SELECT 1 FROM zuse_rust_github_users u WHERE u.actor_id=o.actor_id AND u.epoch=o.actor_epoch)").bind(&id).bind(sealed).bind(now()).execute(&self.pool).await?;
        if count.rows_affected() != 1 {
            return Err(ApiError::conflict("github_operation_unavailable"));
        }
        Ok(())
    }
    async fn fail_operation(&self, id: &str) -> Result<(), ApiError> {
        sqlx::query("UPDATE zuse_rust_github_operations SET phase='failed',sealed_pending=NULL WHERE operation_id=$1").bind(id).execute(&self.pool).await?;
        Ok(())
    }
    pub async fn operation(&self, access: &Access, id: &str) -> Result<Value, ApiError> {
        self.require(access)?;
        let row=sqlx::query("SELECT phase,sealed_pending,expires_at FROM zuse_rust_github_operations WHERE operation_id=$1 AND actor_id=$2 AND owner_id=$3")
            .bind(id).bind(&access.actor.account_id).bind(&access.owner_id).fetch_optional(&self.pool).await?.ok_or_else(||ApiError::not_found("github_operation_not_found"))?;
        let phase: String = row.try_get("phase")?;
        let expires: i64 = row.try_get("expires_at")?;
        let mut result = json!({"operationId":id,"state":if expires<=now()&&phase!="complete"{"expired"}else{&phase},"expiresAt":expires,"installations":[]});
        if phase == "choosing" && expires > now() {
            let sealed: String = row.try_get("sealed_pending")?;
            let pending: Pending = self.open(&format!("github-pending:{id}"), &sealed)?;
            let Some(browser_installation) = pending.browser_installation else {
                result["state"] = json!("authorizing");
                return Ok(result);
            };
            let mut installs = vec![];
            for page in 1..=100 {
                let response = self
                    .get(
                        &format!("/user/installations?per_page=100&page={page}"),
                        &pending.credentials.access_token,
                    )
                    .await?;
                let entries = response["installations"]
                    .as_array()
                    .ok_or_else(unavailable)?;
                for installation in entries {
                    let Some(installation_id) = installation["id"].as_i64().filter(|n| *n > 0)
                    else {
                        continue;
                    };
                    if installation_id != browser_installation {
                        continue;
                    }
                    // A member connects their identity to existing organization links.
                    if !can_manage(access) {
                        let linked:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM zuse_rust_github_installations WHERE owner_id=$1 AND installation_id=$2 AND state<>'removed')").bind(&access.owner_id).bind(installation_id).fetch_one(&self.pool).await?;
                        if !linked {
                            continue;
                        }
                    }
                    installs.push(installation_summary(installation_id, installation)?);
                }
                if entries.len() < 100 {
                    result["installations"] =
                        serde_json::to_value(installs).map_err(|_| unavailable())?;
                    break;
                }
                if page == 100 {
                    return Err(ApiError::unavailable("github_pagination_limit"));
                }
            }
            result["user"] =
                serde_json::to_value(public_user(&pending.profile)?).map_err(|_| unavailable())?;
            result["installUrl"] = json!(format!(
                "https://github.com/apps/{}/installations/new",
                self.config.slug
            ));
        }
        Ok(result)
    }
    pub async fn select(&self, access: &Access, selection: &Selection) -> Result<(), ApiError> {
        self.require(access)?;
        if selection.installation_id <= 0 {
            return Err(ApiError::bad_request("invalid_github_installation"));
        }
        let row=sqlx::query("SELECT sealed_pending,actor_epoch,scope_epoch FROM zuse_rust_github_operations WHERE operation_id=$1 AND actor_id=$2 AND owner_id=$3 AND phase='choosing' AND expires_at>$4")
            .bind(&selection.operation_id).bind(&access.actor.account_id).bind(&access.owner_id).bind(now()).fetch_optional(&self.pool).await?.ok_or_else(||ApiError::conflict("github_operation_unavailable"))?;
        let pending: Pending = self.open(
            &format!("github-pending:{}", selection.operation_id),
            &row.try_get::<String, _>("sealed_pending")?,
        )?;
        if pending.browser_installation != Some(selection.installation_id) {
            return Err(ApiError::forbidden("github_browser_confirmation_required"));
        }
        let metadata = self
            .installation(selection.installation_id)
            .await?
            .filter(|m| m["suspended_at"].is_null())
            .ok_or_else(|| ApiError::forbidden("github_installation_unavailable"))?;
        let _login = metadata["account"]["login"]
            .as_str()
            .filter(|s| valid_component(s))
            .ok_or_else(unavailable)?;
        let allowed = self
            .installation_access(
                access,
                &metadata,
                &pending.profile,
                &pending.credentials.access_token,
            )
            .await?;
        // Verify OAuth identity again; no installation administrator identity is used.
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
        let claim=sqlx::query("UPDATE zuse_rust_github_operations o SET phase='complete',sealed_pending=NULL WHERE operation_id=$1 AND phase='choosing' AND expires_at>$2 AND EXISTS(SELECT 1 FROM zuse_rust_github_users u WHERE u.actor_id=o.actor_id AND u.epoch=o.actor_epoch AND u.state<>'revoking') AND EXISTS(SELECT 1 FROM zuse_rust_github_scopes s WHERE s.owner_id=o.owner_id AND s.epoch=o.scope_epoch)").bind(&selection.operation_id).bind(now()).execute(&mut *tx).await?;
        if claim.rows_affected() != 1 {
            return Err(ApiError::conflict("github_operation_unavailable"));
        }
        if can_manage(access) {
            // An existing member whitelist cannot be broadened by another member.
            sqlx::query("INSERT INTO zuse_rust_github_installations(owner_id,installation_id,metadata,allowed_repositories,state,updated_at) VALUES($1,$2,$3,$4,'active',$5) ON CONFLICT(owner_id,installation_id) DO UPDATE SET metadata=EXCLUDED.metadata,state='active',revision=zuse_rust_github_installations.revision+1,observation_id=NULL,updated_at=EXCLUDED.updated_at,allowed_repositories=CASE WHEN EXCLUDED.allowed_repositories IS NULL THEN NULL ELSE zuse_rust_github_installations.allowed_repositories END")
                .bind(&access.owner_id).bind(selection.installation_id).bind(&metadata).bind(&allowed).bind(now()).execute(&mut *tx).await?;
        } else {
            let exists:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM zuse_rust_github_installations i JOIN zuse_rust_github_scopes s USING(owner_id) WHERE owner_id=$1 AND installation_id=$2 AND state='active' AND s.epoch=$3)").bind(&access.owner_id).bind(selection.installation_id).bind(scope_epoch).fetch_one(&mut *tx).await?;
            if !exists {
                return Err(ApiError::forbidden("github_installation_admin_required"));
            }
        }
        let count=sqlx::query("UPDATE zuse_rust_github_users SET github_user_id=$2,profile=$3,sealed_credentials=$4,state='connected',refresh_uncertain=false,epoch=epoch+1,version=version+1,updated_at=$5 WHERE actor_id=$1 AND epoch=$6 AND state<>'revoking' AND (state='disconnected' OR github_user_id IS NULL OR github_user_id=$2)")
            .bind(&access.actor.account_id).bind(pending.profile["id"].as_i64().ok_or_else(unavailable)?).bind(&pending.profile).bind(sealed).bind(now()).bind(epoch).execute(&mut *tx).await?;
        if count.rows_affected() != 1 {
            return Err(ApiError::conflict("github_identity_changed"));
        }
        self.commit_join_identity(
            &mut tx,
            access,
            &pending,
            &selection.operation_id,
            &metadata,
        )
        .await?;
        tx.commit().await?;
        Ok(())
    }
    pub async fn cancel(&self, access: &Access, id: &str) -> Result<(), ApiError> {
        self.require(access)?;
        sqlx::query("UPDATE zuse_rust_github_operations SET phase='cancelled',sealed_pending=NULL WHERE operation_id=$1 AND actor_id=$2 AND owner_id=$3 AND phase<>'complete'").bind(id).bind(&access.actor.account_id).bind(&access.owner_id).execute(&self.pool).await?;
        Ok(())
    }
}
