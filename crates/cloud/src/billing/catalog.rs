//! Ledger-backed administration; reads never create periods, reconcile usage or emit charges.
use super::{invalid, number};
use crate::{error::ApiError, store::Store};
use serde_json::Value;
use sqlx::{PgPool, Postgres, Row, Transaction};
use std::{collections::HashSet, sync::Arc};
use zuse_protocol::cloud_billing::*;
const MAX_SAFE: i64 = 9_007_199_254_740_991;

/// Dedicated writer connection; the catalog pool stays read-only.
#[derive(Clone)]
pub struct BillingControl {
    pool: PgPool,
    accounts: Arc<HashSet<String>>,
}
impl BillingControl {
    pub fn from_pool(pool: PgPool, accounts: Vec<String>) -> Self {
        Self {
            pool,
            accounts: Arc::new(accounts.into_iter().collect()),
        }
    }
    pub async fn connect(url: &str, accounts: Vec<String>) -> anyhow::Result<Self> {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(4)
            .after_connect(|conn, _| {
                Box::pin(async move {
                    sqlx::query("SET statement_timeout='10s'")
                        .execute(&mut *conn)
                        .await?;
                    sqlx::query("SET lock_timeout='5s'")
                        .execute(&mut *conn)
                        .await?;
                    Ok(())
                })
            })
            .connect(url)
            .await
            .map_err(|_| anyhow::anyhow!("Cannot connect cloud billing writer"))?;
        Ok(Self::from_pool(pool, accounts))
    }
    async fn set_cap(
        &self,
        owner: &str,
        request: BillingCapRequest,
        now: i64,
    ) -> Result<(), ApiError> {
        if !(0..=MAX_SAFE).contains(&request.overage_cap_micros)
            || request.idempotency_key.is_empty()
            || request.idempotency_key.len() > 1024
        {
            return Err(ApiError::bad_request("invalid_billing_cap"));
        }
        if !self.accounts.contains(owner) {
            return Err(ApiError::forbidden("cloud_billing_writes_disabled"));
        }
        let mut tx = self.pool.begin().await?;
        // Independent from workspace lifecycle ownership. Handoff must take this row's exclusive lock.
        let backend:Option<String>=sqlx::query_scalar("SELECT backend FROM zuse_rust_control_owners WHERE resource_kind='billing' AND resource_id=$1 FOR SHARE").bind(owner).fetch_optional(&mut *tx).await?;
        if backend.as_deref() != Some("rust") {
            return Err(ApiError::conflict("cloud_billing_owner_not_rust"));
        }
        let p = period(&mut tx, owner, now, true).await?;
        let id = p["period_id"].as_str().ok_or_else(invalid)?;
        let prior:Option<i64>=sqlx::query_scalar("SELECT overage_cap_micros FROM api_cloud_billing_cap_commands WHERE period_id=$1 AND idempotency_key=$2").bind(id).bind(&request.idempotency_key).fetch_optional(&mut *tx).await?;
        if let Some(prior) = prior {
            if prior != request.overage_cap_micros {
                return Err(ApiError::conflict("billing_cap_idempotency_conflict"));
            }
        } else {
            let incurred:i64=sqlx::query_scalar("SELECT COALESCE(SUM(amount_micros),0)::bigint FROM api_cloud_billing_ledger WHERE period_id=$1 AND kind='overage-charge'").bind(id).fetch_one(&mut *tx).await?;
            if request.overage_cap_micros < incurred {
                return Err(ApiError::conflict("billing_cap_below_incurred"));
            }
            sqlx::query("INSERT INTO api_cloud_billing_cap_commands(period_id,idempotency_key,overage_cap_micros,created_at) VALUES($1,$2,$3,$4)").bind(id).bind(&request.idempotency_key).bind(request.overage_cap_micros).bind(now).execute(&mut *tx).await?;
            sqlx::query("UPDATE api_cloud_billing_periods SET overage_cap_micros=$2,updated_at=$3 WHERE period_id=$1").bind(id).bind(request.overage_cap_micros).bind(now).execute(&mut *tx).await?;
        }
        tx.commit().await?;
        Ok(())
    }
}
async fn period(
    tx: &mut Transaction<'_, Postgres>,
    owner: &str,
    now: i64,
    lock: bool,
) -> Result<Value, ApiError> {
    let sql = if lock {
        "SELECT to_jsonb(p) FROM api_cloud_billing_periods p WHERE account_id=$1 AND period_start<=$2 AND period_end>$2 ORDER BY period_start DESC LIMIT 1 FOR UPDATE"
    } else {
        "SELECT to_jsonb(p) FROM api_cloud_billing_periods p WHERE account_id=$1 AND period_start<=$2 AND period_end>$2 ORDER BY period_start DESC LIMIT 1"
    };
    sqlx::query_scalar(sql)
        .bind(owner)
        .bind(now)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or_else(|| ApiError::conflict("cloud_billing_subscription_required"))
}
fn safe(value: i128) -> Result<i64, ApiError> {
    if value.abs() > i128::from(MAX_SAFE) {
        Err(invalid())
    } else {
        Ok(value as i64)
    }
}
impl Store {
    pub fn with_billing_control(mut self, control: BillingControl) -> Self {
        self.billing = Some(Arc::new(control));
        self
    }
    pub fn billing_writes_enabled(&self) -> bool {
        self.billing.is_some()
    }
    pub async fn billing_set_cap(
        &self,
        owner: &str,
        request: BillingCapRequest,
        now: i64,
    ) -> Result<BillingSummary, ApiError> {
        self.billing
            .as_ref()
            .ok_or_else(|| ApiError::unavailable("cloud_billing_writes_disabled"))?
            .set_cap(owner, request, now)
            .await?;
        self.billing_summary(owner, now).await
    }
    pub async fn billing_summary(&self, owner: &str, now: i64) -> Result<BillingSummary, ApiError> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
            .execute(&mut *tx)
            .await?;
        let p = period(&mut tx, owner, now, false).await?;
        let id = p["period_id"].as_str().ok_or_else(invalid)?;
        let row=sqlx::query("SELECT (SELECT COALESCE(SUM(amount_micros),0)::bigint FROM api_cloud_billing_ledger WHERE period_id=$1 AND kind='provider-cost') AS confirmed,(SELECT COALESCE(SUM(amount_micros),0)::bigint FROM api_cloud_billing_ledger WHERE period_id=$1 AND kind='overage-charge') AS charged,(SELECT COALESCE(SUM(provider_cost_micros),0)::bigint FROM api_cloud_billing_reservations WHERE period_id=$1 AND expires_at>$2) AS reserved,(SELECT COALESCE(SUM(provider_cost_micros),0)::bigint FROM api_cloud_billing_usage WHERE period_id=$1 AND resource_kind='snapshot') AS storage, EXISTS(SELECT 1 FROM api_cloud_billing_usage WHERE period_id=$1 AND status='provisional') OR EXISTS(SELECT 1 FROM api_cloud_billing_reservations WHERE period_id=$1 AND expires_at>$2) AS provisional,(SELECT MAX(created_at) FROM api_cloud_billing_usage WHERE period_id=$1 AND status IN ('confirmed','corrected')) AS provider_at,(SELECT MAX(created_at) FROM api_cloud_billing_meter_reconciliations WHERE period_id=$1 AND provider=$3) AS billing_at").bind(id).bind(now).bind(p["billing_provider"].as_str().unwrap_or("polar")).fetch_one(&mut *tx).await?;
        let totals = (
            row.try_get::<i64, _>("confirmed")?,
            row.try_get::<i64, _>("charged")?,
            row.try_get::<i64, _>("reserved")?,
        );
        let mut result = summary(&p, totals)?;
        result.storage_cost_micros = safe(i128::from(row.try_get::<i64, _>("storage")?))?;
        result.usage_provisional = row.try_get("provisional")?;
        result.last_provider_reconciled_at = row.try_get("provider_at")?;
        result.last_billing_reconciled_at = row.try_get("billing_at")?;
        if p["billing_provider"].as_str().unwrap_or("polar") == "polar" {
            result.last_polar_reconciled_at = result.last_billing_reconciled_at;
        }
        tx.commit().await?;
        Ok(result)
    }
    pub async fn billing_usage(
        &self,
        owner: &str,
        cursor: Option<&str>,
        limit: usize,
        now: i64,
    ) -> Result<BillingUsagePage, ApiError> {
        if cursor.is_some_and(|v| v.is_empty() || v.len() > 4096) {
            return Err(ApiError::bad_request("invalid_billing_cursor"));
        }
        let limit = limit.clamp(1, 100);
        let mut tx = self.pool.begin().await?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
            .execute(&mut *tx)
            .await?;
        let p = period(&mut tx, owner, now, false).await?;
        let rows:Vec<Value>=sqlx::query_scalar("SELECT to_jsonb(items) FROM (SELECT entry_id,resource_kind,resource_id,provider,provider_execution_id,started_at,ended_at,vcpu_count,memory_mib,provider_cost_micros,status FROM api_cloud_billing_usage WHERE period_id=$1 UNION ALL SELECT 'reservation:'||resource_kind||':'||resource_id,resource_kind,resource_id,provider,NULL,started_at,updated_at,vcpu_count,memory_mib,provider_cost_micros,'provisional' FROM api_cloud_billing_reservations WHERE period_id=$1 AND expires_at>$3) items WHERE ($2::text IS NULL OR entry_id<$2) ORDER BY entry_id DESC LIMIT $4").bind(&p["period_id"].as_str().ok_or_else(invalid)?).bind(cursor).bind(now).bind((limit+1) as i64).fetch_all(&mut *tx).await?;
        let has_next = rows.len() > limit;
        let mut items = Vec::new();
        for row in rows.into_iter().take(limit) {
            let text = |key: &str| row[key].as_str().map(str::to_owned).ok_or_else(invalid);
            let kind = text("resource_kind")?;
            if !matches!(kind.as_str(), "workspace" | "build" | "other" | "snapshot") {
                return Err(invalid());
            }
            let status = text("status")?;
            if !matches!(status.as_str(), "provisional" | "confirmed" | "corrected") {
                return Err(invalid());
            }
            items.push(BillingUsageItem {
                entry_id: text("entry_id")?,
                resource_kind: if kind == "snapshot" {
                    "other".into()
                } else {
                    kind.clone()
                },
                usage_kind: (kind == "snapshot").then(|| "snapshot-storage".into()),
                resource_id: text("resource_id")?,
                provider: text("provider")?,
                provider_execution_id: row["provider_execution_id"].as_str().map(str::to_owned),
                started_at: number(&row, "started_at")?,
                ended_at: number(&row, "ended_at")?,
                vcpu_count: number(&row, "vcpu_count")?,
                memory_mib: number(&row, "memory_mib")?,
                provider_cost_micros: safe(i128::from(
                    row["provider_cost_micros"].as_i64().ok_or_else(invalid)?,
                ))?,
                status,
            });
        }
        let next_cursor = has_next.then(|| items.last().unwrap().entry_id.clone());
        tx.commit().await?;
        Ok(BillingUsagePage { items, next_cursor })
    }
}
fn summary(
    p: &Value,
    (confirmed, charged, reserved): (i64, i64, i64),
) -> Result<BillingSummary, ApiError> {
    if p["currency"] != "USD" {
        return Err(invalid());
    }
    let included = number(p, "included_provider_cost_micros")?;
    let markup = number(p, "markup_basis_points")?;
    let cap = number(p, "overage_cap_micros")?;
    let base = number(p, "base_price_micros")?;
    let confirmed = i128::from(confirmed);
    let total = (confirmed + i128::from(reserved)).max(0);
    let overage = (total - i128::from(included)).max(0);
    let charge =
        |cost: i128| (cost - i128::from(included)).max(0) * (10_000 + i128::from(markup)) / 10_000;
    let actual = (i128::from(charged) + (charge(total) - charge(confirmed.max(0))).max(0))
        .min(i128::from(cap));
    let status = match p["status"].as_str() {
        Some("manual") => BillingStatus::Manual,
        Some("ended") => BillingStatus::Ended,
        Some("grace" | "billing-hold") => BillingStatus::BillingHold,
        Some("active" | "pending") => {
            if overage > 0 && actual >= i128::from(cap) {
                BillingStatus::BillingHold
            } else {
                BillingStatus::Active
            }
        }
        _ => return Err(invalid()),
    };
    let used = total.min(i128::from(included));
    Ok(BillingSummary {
        currency: "USD".into(),
        status,
        period_start: number(p, "period_start")?,
        period_end: number(p, "period_end")?,
        base_price_micros: base,
        included_provider_cost_micros: included,
        provider_cost_micros: safe(total)?,
        storage_cost_micros: 0,
        included_used_micros: safe(used)?,
        included_remaining_micros: safe(i128::from(included) - used)?,
        overage_provider_cost_micros: safe(overage)?,
        overage_charge_micros: safe(actual)?,
        overage_cap_micros: cap,
        markup_basis_points: markup,
        current_invoice_estimate_micros: safe(i128::from(base) + actual)?,
        last_provider_reconciled_at: None,
        last_billing_reconciled_at: None,
        last_polar_reconciled_at: None,
        usage_provisional: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn summary_keeps_settled_charge_after_provider_corrections() {
        let p = json!({"currency":"USD","status":"active","period_start":0,"period_end":3600000,"base_price_micros":40000000,"included_provider_cost_micros":35000000,"markup_basis_points":500,"overage_cap_micros":25000000});
        let value = summary(&p, (70_000_000, 10_000_000, 0)).unwrap();
        assert_eq!(value.overage_charge_micros, 10_000_000);
        assert_eq!(value.current_invoice_estimate_micros, 50_000_000);
        // Reservations add only their incremental charge; old confirmed cost is not charged twice.
        let value = summary(&p, (70_000_000, 10_000_000, 1_000_000)).unwrap();
        assert_eq!(value.overage_charge_micros, 11_050_000);
        assert!(summary(&p, (i64::MAX, 0, 0)).is_err());
    }
}
