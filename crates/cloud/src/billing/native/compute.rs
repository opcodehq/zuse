//! Canonical compute observations debit the same wallet as model reservations.
use super::*;
use brokeboy::core::decimal::Decimal;
use sha2::{Digest, Sha256};

impl BillingControl {
    /// Acknowledgement and provider observation remain in the host database; the
    /// native debit's stable key recovers a crash between the two commits.
    pub(super) async fn settle_compute_credits(&self, owner: &str) -> Result<(), ApiError> {
        let Some(native) = self.native_for(owner).await? else {
            return Ok(());
        };
        for _ in 0..100 {
            let mut tx = self.owned_transaction(owner).await?;
            let row:Option<(String,i64,String)>=sqlx::query_as("SELECT entry_id,amount_micros,period_id FROM zuse_brokeboy_compute_charges WHERE account_id=$1 AND state='pending' ORDER BY created_at,entry_id LIMIT 1 FOR UPDATE SKIP LOCKED").bind(owner).fetch_optional(&mut *tx).await?;
            let Some((entry, cost, period)) = row else {
                tx.commit().await?;
                break;
            };
            let end: i64 = sqlx::query_scalar(
                "SELECT period_end FROM api_cloud_billing_periods WHERE period_id=$1",
            )
            .bind(&period)
            .fetch_one(&mut *tx)
            .await?;
            if end <= chrono::Utc::now().timestamp_millis() {
                // A late observation must not silently spend a new month's grant.
                sqlx::query("UPDATE zuse_brokeboy_compute_charges SET state='reconciliation' WHERE account_id=$1 AND entry_id=$2").bind(owner).bind(&entry).execute(&mut *tx).await?;
                tx.commit().await?;
                continue;
            }
            let amount = Decimal::from_scaled(i128::from(cost))
                .map_err(failure)?
                .to_string();
            if cost > 0 {
                let result=native.engine.call("usage.consume",json!({"customerId":owner,"feature":CREDIT_WALLET,"amount":amount,"properties":{"kind":"compute","entryId":entry,"periodId":period},"idempotencyKey":format!("compute:{:x}",Sha256::digest(entry.as_bytes()))})).await;
                match result {
                    Ok(_) => {}
                    Err(error) if error.code == "insufficient_balance" => {
                        tx.rollback().await?;
                        break;
                    }
                    Err(error) => return Err(failure(error)),
                }
            }
            sqlx::query("UPDATE zuse_brokeboy_compute_charges SET state='paid' WHERE account_id=$1 AND entry_id=$2").bind(owner).bind(&entry).execute(&mut *tx).await?;
            tx.commit().await?;
        }
        self.sync_credit_capacity(owner).await
    }
    pub(in crate::billing) async fn shared_available_micros(
        &self,
        owner: &str,
    ) -> Result<i64, ApiError> {
        let native = self
            .native_for(owner)
            .await?
            .ok_or_else(|| ApiError::unavailable("embedded_billing_disabled"))?;
        let value = native
            .engine
            .call(
                "credits.balance",
                json!({"customerId":owner,"feature":CREDIT_WALLET}),
            )
            .await
            .map_err(failure)?;
        let available = text(&value, "available")?
            .parse::<Decimal>()
            .map_err(failure)?
            .scaled();
        let pending:i64=sqlx::query_scalar("SELECT COALESCE(SUM(amount_micros),0)::bigint FROM zuse_brokeboy_compute_charges WHERE account_id=$1 AND state IN ('pending','reconciliation')").bind(owner).fetch_one(&self.pool).await?;
        i64::try_from((available - i128::from(pending)).max(0))
            .map_err(|_| ApiError::unavailable("invalid_billing_result"))
    }
    pub(super) async fn sync_credit_capacity(&self, owner: &str) -> Result<(), ApiError> {
        let native = self
            .native_for(owner)
            .await?
            .ok_or_else(|| ApiError::unavailable("embedded_billing_disabled"))?;
        let mut tx = self.owned_transaction(owner).await?;
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,68131))")
            .bind(owner)
            .execute(&mut *tx)
            .await?;
        let balance = native
            .engine
            .call(
                "credits.balance",
                json!({"customerId":owner,"feature":CREDIT_WALLET}),
            )
            .await
            .map_err(failure)?;
        let available = text(&balance, "available")?
            .parse::<Decimal>()
            .map_err(failure)?
            .scaled();
        let pending:i64=sqlx::query_scalar("SELECT COALESCE(SUM(amount_micros),0)::bigint FROM zuse_brokeboy_compute_charges WHERE account_id=$1 AND state IN ('pending','reconciliation')").bind(owner).fetch_one(&mut *tx).await?;
        let available = (available - i128::from(pending)).max(0);
        let review:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM zuse_brokeboy_compute_charges WHERE account_id=$1 AND state='reconciliation')").bind(owner).fetch_one(&mut *tx).await?;
        sqlx::query("UPDATE api_cloud_billing_periods SET shared_credit_hold=$2 WHERE account_id=$1 AND shared_credit_account").bind(owner).bind(available==0 || review).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(())
    }
}
