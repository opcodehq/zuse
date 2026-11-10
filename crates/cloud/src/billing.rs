//! Ledger-backed billing and explicitly owned provider operations.
mod catalog;
mod delivery;
mod exports;
mod settlement;
mod storage;
pub use exports::{ExportReport, MeterEvent, MeterExportConfig};
pub use settlement::{BoxdSettlementConfig, SettlementReport};
pub use storage::{SnapshotSettlementConfig, SnapshotSettlementReport};
pub mod native;
mod payments;
use crate::error::ApiError;
pub use catalog::BillingControl;
pub use payments::{
    CheckoutSummary, CreditDispute, CreditEvent, CreditPurchase, CustomerPage, PaymentEvent,
    PaymentProviders, PortalProvider, Subscription,
};
use serde_json::Value;
use sqlx::{Postgres, Transaction};

pub(crate) async fn require_capacity(
    tx: &mut Transaction<'_, Postgres>,
    account: &str,
    now: i64,
) -> Result<(), ApiError> {
    let period:Option<Value>=sqlx::query_scalar("SELECT to_jsonb(p) FROM api_cloud_billing_periods p WHERE account_id=$1 AND period_start<=$2 AND period_end>$2 ORDER BY period_start DESC LIMIT 1 FOR SHARE")
        .bind(account).bind(now).fetch_optional(&mut **tx).await?;
    let period = period.ok_or_else(|| ApiError::forbidden("cloud_billing_period_missing"))?;
    if period["currency"] != "USD" {
        return Err(ApiError::unavailable("cloud_billing_policy_invalid"));
    }
    let id = period["period_id"].as_str().ok_or_else(invalid)?;
    let totals:(i64,i64,i64)=sqlx::query_as("SELECT (SELECT COALESCE(SUM(amount_micros),0)::bigint FROM api_cloud_billing_ledger WHERE period_id=$1 AND kind='provider-cost'),(SELECT COALESCE(SUM(amount_micros),0)::bigint FROM api_cloud_billing_ledger WHERE period_id=$1 AND kind='overage-charge'),(SELECT COALESCE(SUM(provider_cost_micros),0)::bigint FROM api_cloud_billing_reservations WHERE period_id=$1 AND expires_at>$2)")
        .bind(id).bind(now).fetch_one(&mut **tx).await?;
    if held(
        period["status"].as_str().ok_or_else(invalid)?,
        number(&period, "included_provider_cost_micros")?,
        number(&period, "markup_basis_points")?,
        number(&period, "overage_cap_micros")?,
        totals,
    )? {
        return Err(ApiError::forbidden("cloud_billing_hold"));
    }
    Ok(())
}
/// Apply the same cumulative policy to facts read by the locked admission batch.
pub(crate) fn validate_capacity_snapshot(
    period: Option<Value>,
    totals: &Value,
) -> Result<(), ApiError> {
    let period = period.ok_or_else(|| ApiError::forbidden("cloud_billing_period_missing"))?;
    if period["currency"] != "USD" {
        return Err(invalid());
    }
    period["period_id"].as_str().ok_or_else(invalid)?;
    let totals: (i64, i64, i64) = serde_json::from_value(totals.clone()).map_err(|_| invalid())?;
    if held(
        period["status"].as_str().ok_or_else(invalid)?,
        number(&period, "included_provider_cost_micros")?,
        number(&period, "markup_basis_points")?,
        number(&period, "overage_cap_micros")?,
        totals,
    )? {
        return Err(ApiError::forbidden("cloud_billing_hold"));
    }
    Ok(())
}
fn invalid() -> ApiError {
    ApiError::unavailable("cloud_billing_policy_invalid")
}
fn number(value: &Value, key: &str) -> Result<i64, ApiError> {
    value[key]
        .as_i64()
        .filter(|n| (0..=9_007_199_254_740_991).contains(n))
        .ok_or_else(invalid)
}
/// Original cumulative policy: reserved cost consumes allowance; already settled charge is authoritative.
fn held(
    status: &str,
    included: i64,
    markup: i64,
    cap: i64,
    totals: (i64, i64, i64),
) -> Result<bool, ApiError> {
    cost_policy(status, included, markup, cap)?
        .held(totals.0, totals.1, totals.2)
        .map_err(|_| invalid())
}
/// Shared monetary policy, evaluated inside the host's existing admission transaction.
/// Mutable cap terms stay in the host ledger rather than creating a second authority.
fn cost_policy(
    status: &str,
    included: i64,
    markup_basis_points: i64,
    overage_cap: i64,
) -> Result<brokeboy::core::periods::CostPolicy, ApiError> {
    let status = serde_json::from_value(serde_json::json!(status)).map_err(|_| invalid())?;
    let policy = brokeboy::core::periods::CostPolicy {
        status,
        included,
        markup_basis_points,
        overage_cap,
    };
    policy.validate().map_err(|_| invalid())?;
    Ok(policy)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn capacity_matches_cumulative_ledger_and_live_reservations() {
        assert!(!held("active", 35_000_000, 500, 25_000_000, (35_000_000, 0, 0)).unwrap());
        assert!(held("active", 35_000_000, 500, 0, (35_000_000, 0, 1)).unwrap());
        assert!(
            held(
                "active",
                35_000_000,
                500,
                1_050_000,
                (35_000_000, 0, 1_000_000)
            )
            .unwrap()
        );
        assert!(
            !held(
                "active",
                35_000_000,
                500,
                1_050_001,
                (35_000_000, 0, 1_000_000)
            )
            .unwrap()
        );
        // Corrections and already charged money must not be recalculated from current raw cost.
        assert!(
            !held(
                "active",
                35_000_000,
                500,
                25_000_000,
                (70_000_000, 10_000_000, 0)
            )
            .unwrap()
        );
        assert!(held("grace", 35_000_000, 500, 25_000_000, (0, 0, 0)).unwrap());
        assert!(!held("manual", 35_000_000, 500, 0, (i64::MAX, 0, i64::MAX)).unwrap());
        assert!(held("unknown", 35_000_000, 500, 25_000_000, (0, 0, 0)).is_err());
    }
}
