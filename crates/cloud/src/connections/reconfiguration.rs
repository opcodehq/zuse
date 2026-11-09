//! Explicit replacement proposals keep old credentials until leased verification commits.
use super::*;
#[derive(serde::Serialize, serde::Deserialize)]
pub(super) struct Proposal {
    pub version: u32,
    pub connection: String,
    pub owner: String,
    pub agent: String,
    pub method: String,
    pub base_version: i64,
    pub base_epoch: i64,
    pub secret: String,
}
impl Drop for Proposal {
    fn drop(&mut self) {
        use zeroize::Zeroize;
        self.secret.zeroize();
    }
}
pub(super) const PREFIX: &str = "auth_cfg_";
// Reserved authorizing rows cannot be claimed by an old queued-only worker.
pub(super) const WAITING_LEASE: &str = "credential-proposal-v1";
impl ConnectionControl {
    pub(super) async fn configure_existing(
        &self,
        owner: &str,
        actor: &str,
        request: &ConfigureConnection,
        secret: &str,
        now: i64,
    ) -> Result<AgentConnection, ApiError> {
        let id = request.connection_id.as_deref().ok_or_else(unavailable)?;
        if id.is_empty() || id.len() > 512 || id.chars().any(char::is_control) {
            return Err(ApiError::bad_request("invalid_agent_connection"));
        }
        let operation = format!(
            "{PREFIX}{}",
            self.master.digest_api(
                &format!(
                    "agent-reconfigure:{owner}:{id}:{}:{}:{}",
                    request.agent.as_str(),
                    request.method.as_str(),
                    request.idempotency_key
                ),
                secret
            )
        );
        let key = format!("verify:{}", request.idempotency_key);
        let mut tx = self.pool.begin().await?;
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
            .bind(format!("connection:{owner}:{}", request.idempotency_key))
            .execute(&mut *tx)
            .await?;
        let row:Value=sqlx::query_scalar("SELECT to_jsonb(c) FROM cloud_agent_connections c WHERE owner_id=$1 AND connection_id=$2 FOR UPDATE").bind(owner).bind(id).fetch_optional(&mut *tx).await?.ok_or_else(||ApiError::not_found("agent_connection_not_found"))?;
        if row["state"] == "pending" {
            return Err(ApiError::conflict(
                "agent_connection_verification_in_progress",
            ));
        }
        if row["agent"] != request.agent.as_str()
            || row["method"] != request.method.as_str()
            || row["state"] == "revoked"
        {
            return Err(ApiError::not_found("agent_connection_not_found"));
        }
        let prior:Option<(String,String)>=sqlx::query_as("SELECT operation_id,connection_id FROM cloud_auth_operations WHERE owner_id=$1 AND idempotency_key=$2 FOR UPDATE").bind(owner).bind(&key).fetch_optional(&mut *tx).await?;
        if let Some((prior, connection)) = prior {
            if prior != operation || connection != id {
                return Err(ApiError::conflict("agent_connection_idempotency_conflict"));
            }
        } else {
            sqlx::query("UPDATE cloud_auth_operations SET state='expired',operation_ciphertext=NULL,lease_id=NULL,lease_expires_at=NULL,updated_at=$3 WHERE owner_id=$1 AND connection_id=$2 AND state IN ('queued','authorizing') AND expires_at<=$3").bind(owner).bind(id).bind(now).execute(&mut *tx).await?;
            let pending:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM cloud_auth_operations WHERE owner_id=$1 AND connection_id=$2 AND state IN ('queued','authorizing'))").bind(owner).bind(id).fetch_one(&mut *tx).await?;
            if pending {
                return Err(ApiError::conflict(
                    "agent_connection_verification_in_progress",
                ));
            }
            let proposal = Proposal {
                version: 1,
                connection: id.into(),
                owner: owner.into(),
                agent: request.agent.as_str().into(),
                method: request.method.as_str().into(),
                base_version: row["version"].as_i64().ok_or_else(unavailable)?,
                base_epoch: row["authority_epoch"].as_i64().ok_or_else(unavailable)?,
                secret: secret.into(),
            };
            let plaintext =
                Zeroizing::new(serde_json::to_string(&proposal).map_err(|_| unavailable())?);
            let encrypted = self
                .master
                .seal_api(&format!("agent-login:{owner}:{operation}"), &plaintext)
                .map_err(|_| unavailable())?;
            sqlx::query("INSERT INTO cloud_auth_operations(operation_id,owner_id,actor_id,connection_id,idempotency_key,state,operation_ciphertext,expires_at,created_at,updated_at,lease_id,lease_expires_at) VALUES($1,$2,$3,$4,$5,'authorizing',$6,$7,GREATEST($8,COALESCE((SELECT max(created_at)+1 FROM cloud_auth_operations WHERE connection_id=$4),$8)),$8,'credential-proposal-v1',$7)").bind(&operation).bind(owner).bind(actor).bind(id).bind(key).bind(encrypted).bind(now+120_000).bind(now).execute(&mut *tx).await?;
        }
        tx.commit().await?;
        let mut result = public(row)?;
        result.credential_update_key = Some(request.idempotency_key.clone());
        result.credential_update = Some(self.operation(owner, &operation, now).await?);
        if result.credential_update.as_ref().is_some_and(|operation| {
            matches!(
                operation.state,
                LoginState::Queued | LoginState::Authorizing
            )
        }) {
            result.state = ConnectionState::Pending;
        }
        Ok(result)
    }
}

#[cfg(test)]
#[path = "reconfiguration_tests.rs"]
mod tests;
