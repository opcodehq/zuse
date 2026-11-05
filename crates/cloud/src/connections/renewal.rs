//! Explicit staging-only authority operation. Never invokes a model or fabricates rejection.
use super::{ConnectionControl, worker::AuthDriver};
use crate::error::ApiError;
use serde::Serialize;
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RenewalReceipt {
    pub agent: &'static str,
    pub source: &'static str,
    pub connection_id: String,
    pub version_before: i64,
    pub version_after: i64,
    pub authority_epoch: i64,
    pub expires_at: Option<i64>,
    pub same_account: bool,
}
impl ConnectionControl {
    pub async fn renew_codex_connection(
        &self,
        owner: &str,
        connection: &str,
        driver: &dyn AuthDriver,
    ) -> Result<RenewalReceipt, ApiError> {
        self.allowed(owner)?;
        if self.accounts.len() != 1 {
            return Err(ApiError::forbidden(
                "credential_renewal_exact_owner_required",
            ));
        }
        let schema: String = sqlx::query_scalar("SELECT current_schema()")
            .fetch_one(&self.pool)
            .await?;
        if schema != "zuse_staging"
            || std::env::var("ZUSE_DEPLOYMENT_ENVIRONMENT").as_deref() != Ok("staging")
        {
            return Err(ApiError::forbidden("credential_renewal_staging_required"));
        }
        let before:Option<(i64,i64,Option<String>)>=sqlx::query_as("SELECT version,authority_epoch,provider_account_id FROM cloud_agent_connections WHERE owner_id=$1 AND connection_id=$2 AND agent='codex' AND method='device-code' AND state='connected'").bind(owner).bind(connection).fetch_optional(&self.pool).await?;
        let (version_before, epoch, account) = before
            .ok_or_else(|| ApiError::conflict("credential_renewal_device_connection_required"))?;
        if account.as_deref().is_none_or(str::is_empty) {
            return Err(ApiError::conflict("credential_renewal_account_required"));
        }
        // `force` is an explicit maintenance request, not a forged vendor401.
        let refreshed = self
            .credential(owner, connection, "codex", true, driver)
            .await?;
        if refreshed.authority_epoch != epoch || refreshed.account_id != account {
            return Err(ApiError::conflict("credential_renewal_fence_changed"));
        }
        drop(refreshed);
        let after:Option<(i64,i64,Option<String>,Option<i64>)>=sqlx::query_as("SELECT version,authority_epoch,provider_account_id,expires_at FROM cloud_agent_connections WHERE owner_id=$1 AND connection_id=$2 AND agent='codex' AND method='device-code' AND state='connected'").bind(owner).bind(connection).fetch_optional(&self.pool).await?;
        let (version_after, after_epoch, after_account, expires_at) =
            after.ok_or_else(|| ApiError::conflict("credential_renewal_fence_changed"))?;
        if version_after <= version_before || after_epoch != epoch || after_account != account {
            return Err(ApiError::conflict("credential_renewal_fence_changed"));
        }
        Ok(RenewalReceipt {
            agent: "codex",
            source: "native-account-refresh",
            connection_id: connection.into(),
            version_before,
            version_after,
            authority_epoch: epoch,
            expires_at,
            same_account: true,
        })
    }
}
