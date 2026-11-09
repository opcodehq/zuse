//! Existing verified authority identities may explicitly attach a further
//! installation without authorizing the same GitHub account again. This path
//! cannot introduce, swap, or import a previously unverified identity.
use super::*;
impl NativeGithub {
    pub async fn connect_existing(
        &self,
        access: &Access,
        installation: i64,
    ) -> Result<Value, ApiError> {
        self.require(access)?;
        if installation <= 0 {
            return Err(ApiError::bad_request("invalid_github_installation"));
        }
        let actor = &access.actor.account_id;
        let credentials = self.credentials(actor).await?;
        let (epoch, version, profile) = self
            .credential_binding(actor, &credentials.access_token)
            .await?;
        let verified = self.get("/user", &credentials.access_token).await?;
        if verified["id"] != profile["id"] {
            return Err(ApiError::forbidden("github_identity_changed"));
        }
        // A per-user installation listing proves visibility. Membership/selected
        // repository permissions are checked again by the same finalizer as OAuth.
        let mut visible = false;
        let mut complete = false;
        for page in 1..=100 {
            let response = self
                .get(
                    &format!("/user/installations?per_page=100&page={page}"),
                    &credentials.access_token,
                )
                .await?;
            let installations = response["installations"]
                .as_array()
                .filter(|v| v.len() <= 100)
                .ok_or_else(unavailable)?;
            if installations
                .iter()
                .any(|i| i["id"] == installation && i["suspended_at"].is_null())
            {
                visible = true;
                break;
            }
            if installations.len() < 100 {
                complete = true;
                break;
            }
        }
        if !visible {
            return Err(if complete {
                ApiError::forbidden("github_installation_access_denied")
            } else {
                ApiError::unavailable("github_pagination_limit")
            });
        }
        let id = uuid::Uuid::new_v4().to_string();
        let pending = Pending {
            credentials,
            profile,
            browser_installation: Some(installation),
        };
        let sealed = self.seal(&format!("github-pending:{id}"), &pending)?;
        let mut tx = self.pool.begin().await?;
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
            .bind(format!("github-user:{actor}"))
            .execute(&mut *tx)
            .await?;
        let connected:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM zuse_rust_github_users WHERE actor_id=$1 AND epoch=$2 AND version=$3 AND state='connected' AND NOT refresh_uncertain)").bind(actor).bind(epoch).bind(version).fetch_one(&mut *tx).await?;
        if !connected {
            return Err(reconnect());
        }
        sqlx::query(
            "INSERT INTO zuse_rust_github_scopes(owner_id) VALUES($1) ON CONFLICT DO NOTHING",
        )
        .bind(&access.owner_id)
        .execute(&mut *tx)
        .await?;
        let scope_epoch: i64 =
            sqlx::query_scalar("SELECT epoch FROM zuse_rust_github_scopes WHERE owner_id=$1")
                .bind(&access.owner_id)
                .fetch_one(&mut *tx)
                .await?;
        // Random, undisclosed state cannot be used to open a new browser transaction.
        sqlx::query("INSERT INTO zuse_rust_github_operations(operation_id,state_hash,actor_id,owner_id,actor_epoch,scope_epoch,phase,sealed_pending,expires_at,created_at) VALUES($1,$2,$3,$4,$5,$6,'choosing',$7,$8,$9)").bind(&id).bind(hash(&opaque())).bind(actor).bind(&access.owner_id).bind(epoch).bind(scope_epoch).bind(sealed).bind(now()+600000).bind(now()).execute(&mut *tx).await?;
        tx.commit().await?;
        self.select(
            access,
            &Selection {
                operation_id: id.clone(),
                installation_id: installation,
            },
        )
        .await?;
        Ok(json!({"operationId":id,"state":"complete","reusedIdentity":true}))
    }
}
impl NativeGithub {
    /// Called in the OAuth finalization transaction, after browser consent, fresh
    /// user proof and immutable enrollment checks. Unique IDs prevent identity
    /// reassignment across accounts even when their task scopes differ.
    pub(in crate::github::native) async fn commit_join_identity(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        access: &Access,
        pending: &Pending,
        verification: &str,
        installation: &Value,
    ) -> Result<(), ApiError> {
        let exists: bool =
            sqlx::query_scalar("SELECT to_regclass('api_github_identities') IS NOT NULL")
                .fetch_one(&mut **tx)
                .await?;
        if !exists {
            return Ok(());
        }
        let identity = VerifiedIdentity {
            account_id: access.actor.account_id.clone(),
            github_user_id: pending.profile["id"]
                .as_i64()
                .filter(|id| *id > 0)
                .ok_or_else(unavailable)?,
            login: pending.profile["login"]
                .as_str()
                .ok_or_else(unavailable)?
                .into(),
            organization_ids: if installation["account"]["type"] == "Organization" {
                vec![
                    installation["account"]["id"]
                        .as_i64()
                        .ok_or_else(unavailable)?,
                ]
            } else {
                vec![]
            },
            verification_id: verification.into(),
        };
        let result=sqlx::query("INSERT INTO api_github_identities(account_id,github_user_id,data) VALUES($1,$2,$3) ON CONFLICT(account_id) DO UPDATE SET github_user_id=EXCLUDED.github_user_id,data=EXCLUDED.data").bind(&identity.account_id).bind(identity.github_user_id).bind(json!(identity)).execute(&mut **tx).await;
        match result {
            Ok(_) => Ok(()),
            Err(sqlx::Error::Database(e)) if e.is_unique_violation() => {
                Err(ApiError::conflict("github_identity_already_linked"))
            }
            Err(e) => Err(e.into()),
        }?;
        let configured: bool =
            sqlx::query_scalar("SELECT to_regclass('zuse_rust_github_join_jobs') IS NOT NULL")
                .fetch_one(&mut **tx)
                .await?;
        if configured {
            let ids:Vec<i64>=sqlx::query_scalar("SELECT DISTINCT p.installation_id FROM api_github_join_policies p JOIN api_github_org_members r USING(installation_id) WHERE r.github_user_id=$1").bind(identity.github_user_id).fetch_all(&mut **tx).await?;
            for id in ids {
                self.commit_membership_jobs(tx, "membership", &json!({"installation":{"id":id}}))
                    .await?;
            }
        }
        Ok(())
    }
}
