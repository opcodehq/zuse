//! Hosted-model requests hold wallet credit while they stream and spend what they
//! used. Usage is recorded before it is settled, so a lost settlement is retried
//! with the same key and never charges twice.
use super::*;
use serde::Serialize;

pub const MIGRATION: &str = include_str!("../../../migrations/0032_hosted_models.sql");
/// A request still open after this lost its stream (process exit); it bills its estimate.
const ABANDONED_MS: i64 = 3_600_000;
/// Outlives abandonment, so the worker settles a lost stream before its hold lapses.
const HOLD_MS: i64 = 2 * ABANDONED_MS;

#[derive(Default, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostedUsageReport {
    pub abandoned: u64,
    pub settled: usize,
    pub deferred: usize,
    pub errors: Vec<String>,
}

impl BillingControl {
    fn hosted_wallet(&self) -> Result<&crate::billing::Wallet, ApiError> {
        self.wallet
            .as_deref()
            .ok_or_else(|| ApiError::unavailable("hosted_models_billing_unavailable"))
    }
    pub(crate) async fn hosted_spendable_micros(&self, owner: &str) -> Result<i64, ApiError> {
        self.hosted_wallet()?.spendable_micros(owner).await
    }
    pub(crate) async fn open_model_request(
        &self,
        owner: &str,
        request: &str,
        model: &str,
        estimate_micros: i64,
        hold_micros: i64,
        now: i64,
    ) -> Result<(), ApiError> {
        let wallet = self.hosted_wallet()?;
        // Proves billing ownership. The wallet has its own pool, so nothing may wait on this lock.
        self.owned_transaction(owner).await?.commit().await?;
        let reservation = wallet
            .hold(owner, request, hold_micros, now + HOLD_MS)
            .await?;
        sqlx::query("INSERT INTO zuse_rust_model_usage(request_id,account_id,model_id,reservation_id,estimate_micros,hold_micros,started_at) VALUES($1,$2,$3,$4,$5,$6,$7)")
            .bind(request).bind(owner).bind(model).bind(reservation).bind(estimate_micros).bind(hold_micros).bind(now)
            .execute(&self.pool).await?;
        Ok(())
    }
    pub(crate) async fn close_model_request(
        &self,
        request: &str,
        tokens: (i64, i64),
        cost_micros: i64,
        estimated: bool,
        now: i64,
    ) -> Result<(), ApiError> {
        sqlx::query("UPDATE zuse_rust_model_usage SET input_tokens=$2,output_tokens=$3,cost_micros=$4,estimated=$5,finished_at=$6 WHERE request_id=$1 AND finished_at IS NULL")
            .bind(request).bind(tokens.0).bind(tokens.1).bind(cost_micros).bind(estimated).bind(now)
            .execute(&self.pool).await?;
        if let Err(error) = self.settle_model_request(request, now).await {
            // The recorded cost stays unsettled, and the billing worker retries it.
            tracing::warn!(code = error.code, %request, "Hosted model settlement deferred");
        }
        Ok(())
    }
    async fn settle_model_request(&self, request: &str, now: i64) -> Result<bool, ApiError> {
        let row: Option<(String, String, i64, i64)> = sqlx::query_as("SELECT account_id,reservation_id,hold_micros,cost_micros FROM zuse_rust_model_usage WHERE request_id=$1 AND finished_at IS NOT NULL AND settled_at IS NULL")
            .bind(request).fetch_optional(&self.pool).await?;
        let Some((owner, reservation, hold, cost)) = row else {
            return Ok(false);
        };
        let wallet = self.hosted_wallet()?;
        let mut charge = cost;
        if cost > hold
            && !wallet
                .increase(&owner, &reservation, request, cost - hold)
                .await?
        {
            tracing::warn!(%request, shortfall = cost - hold, "Hosted model cost exceeded the wallet");
            charge = hold;
        }
        if !wallet.settle(&owner, &reservation, request, charge).await? {
            tracing::warn!(%request, "Hosted model settlement needs reconciliation");
        }
        sqlx::query("UPDATE zuse_rust_model_usage SET settled_at=$2 WHERE request_id=$1 AND settled_at IS NULL")
            .bind(request).bind(now).execute(&self.pool).await?;
        Ok(true)
    }
    /// One billing-worker pass: settle hosted usage, then advance the ledger's own queue.
    pub async fn maintain_wallet_once(&self, now: i64) -> Result<HostedUsageReport, ApiError> {
        let report = self.settle_hosted_models_once(now).await?;
        self.hosted_wallet()?.maintain(100).await?;
        Ok(report)
    }
    async fn settle_hosted_models_once(&self, now: i64) -> Result<HostedUsageReport, ApiError> {
        let mut report = HostedUsageReport {
            abandoned: sqlx::query("UPDATE zuse_rust_model_usage SET cost_micros=estimate_micros,estimated=true,finished_at=$1 WHERE finished_at IS NULL AND started_at<$2")
                .bind(now).bind(now - ABANDONED_MS)
                .execute(&self.pool).await?.rows_affected(),
            ..Default::default()
        };
        let pending: Vec<(String, String)> = sqlx::query_as("SELECT request_id,account_id FROM zuse_rust_model_usage WHERE finished_at IS NOT NULL AND settled_at IS NULL ORDER BY finished_at LIMIT 500")
            .fetch_all(&self.pool).await?;
        for (request, owner) in pending {
            if !self.accounts.contains(&owner) {
                continue;
            }
            match self.settle_model_request(&request, now).await {
                Ok(true) => report.settled += 1,
                Ok(false) => {}
                Err(error) => {
                    report.deferred += 1;
                    report.errors.push(format!("{request}:{}", error.code));
                }
            }
        }
        Ok(report)
    }
}

#[cfg(test)]
mod tests;
