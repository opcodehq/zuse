//! Export existing, authoritative outboxes. Telemetry is never an invoice charge.
use super::{BillingControl, payments};
use crate::error::ApiError;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{Postgres, Row, Transaction};
use std::{collections::BTreeMap, sync::Arc};
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MeterEvent {
    pub account_id: String,
    pub event_name: String,
    pub units: i64,
    pub idempotency_key: String,
    pub occurred_at_ms: i64,
    #[serde(default)]
    pub metadata: BTreeMap<String, String>,
}
fn invalid() -> ApiError {
    ApiError::conflict("invalid_billing_meter_event")
}
fn reconciliation() -> ApiError {
    ApiError::conflict("billing_meter_reconciliation_required")
}
impl MeterEvent {
    pub(super) fn validate(&self) -> Result<(), ApiError> {
        if self.account_id.is_empty()
            || self.account_id.len() > 1024
            || self.idempotency_key.is_empty()
            || self.idempotency_key.len() > 4096
            || self.occurred_at_ms < 0
            || self.occurred_at_ms > 9_007_199_254_740_991
            || self.units.unsigned_abs() > 9_007_199_254_740_991
            || !matches!(
                self.event_name.as_str(),
                "zuse_cloud_overage_cent"
                    | "zuse_cloud_runtime_observed_ms"
                    | "zuse_cloud_provider_cost_micros"
            )
        {
            return Err(invalid());
        }
        if self.event_name != "zuse_cloud_overage_cent"
            && self.metadata.get("billable").map(String::as_str) != Some("false")
        {
            return Err(invalid());
        }
        Ok(())
    }
}
#[derive(Clone, Default)]
pub struct MeterExportConfig {
    pub financial: bool,
    pub telemetry: bool,
    pub meters: BTreeMap<String, String>,
}
impl MeterExportConfig {
    pub fn from_env() -> anyhow::Result<Self> {
        let financial = std::env::var("CLOUD_BILLING_EXPORT_ENABLED").as_deref() == Ok("true");
        let telemetry = std::env::var("CLOUD_USAGE_EXPORT_ENABLED").as_deref() == Ok("true");
        let mut meters = BTreeMap::new();
        if let Ok(id) = std::env::var("STRIPE_CLOUD_OVERAGE_METER_ID") {
            if !id.trim().is_empty() {
                meters.insert("stripe".into(), id);
            }
        }
        anyhow::ensure!(
            !financial || !meters.is_empty(),
            "Financial export requires configured overage meters"
        );
        Ok(Self {
            financial,
            telemetry,
            meters,
        })
    }
    pub fn enabled(&self) -> bool {
        self.financial || self.telemetry
    }
}
#[derive(Default, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportReport {
    pub enabled: bool,
    pub financial: usize,
    pub telemetry: usize,
    pub reconciled: usize,
    pub retried: usize,
}
impl BillingControl {
    pub fn with_meter_exports(mut self, config: MeterExportConfig) -> Self {
        self.meter_exports = Some(Arc::new(config));
        self
    }
    async fn export_transaction(&self, owner: &str) -> Result<Transaction<'_, Postgres>, ApiError> {
        let mut tx = self.owned_transaction(owner).await?;
        let writer:Option<String>=sqlx::query_scalar("SELECT backend FROM zuse_rust_control_owners WHERE resource_kind='billing-export' AND resource_id=$1 FOR SHARE").bind(owner).fetch_optional(&mut *tx).await?;
        if writer.as_deref() != Some("rust") {
            return Err(ApiError::conflict("billing_export_owner_not_rust"));
        }
        Ok(tx)
    }
    async fn send_meter(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        provider_id: &str,
        event: &MeterEvent,
        now: i64,
    ) -> Result<(), ApiError> {
        event.validate()?;
        if event.units == 0 {
            return Ok(());
        }
        let provider = self
            .payments
            .as_ref()
            .and_then(|p| p.providers.get(provider_id))
            .ok_or_else(|| ApiError::unavailable("billing_provider_unavailable"))?;
        let customer: Option<String> = if provider_id == "stripe" {
            sqlx::query_scalar("SELECT customer_id FROM api_stripe_customers WHERE account_id=$1")
                .bind(&event.account_id)
                .fetch_optional(&mut **tx)
                .await?
                .flatten()
        } else {
            None
        };
        if provider_id == "stripe" {
            let customer = customer.as_deref().ok_or_else(reconciliation)?;
            provider
                .verify_customer(&event.account_id, customer)
                .await?;
            let (key, payload) = payments::stripe_meter_command(event, customer, now)?;
            self.deliver_stripe(&key, &payload, now, reconciliation, || {
                provider.report_meter(event, Some(customer))
            })
            .await
        } else {
            provider.report_meter(event, None).await
        }
    }
    async fn financial_export(&self, id: &str, owner: &str, now: i64) -> Result<bool, ApiError> {
        let mut tx = self.export_transaction(owner).await?;
        let row=sqlx::query("SELECT o.*,p.account_id AS period_owner,p.billing_provider,p.currency,p.provider_subscription_id,p.period_start,p.period_end FROM api_cloud_billing_outbox o JOIN api_cloud_billing_periods p ON p.period_id=o.period_id WHERE o.outbox_id=$1 AND o.account_id=$2 AND o.acknowledged_at IS NULL AND o.next_attempt_at<=$3 FOR UPDATE OF o SKIP LOCKED FOR SHARE OF p").bind(id).bind(owner).bind(now).fetch_optional(&mut *tx).await?;
        let Some(row) = row else {
            tx.commit().await?;
            return Ok(false);
        };
        let provider: String = row.try_get("provider")?;
        let period: String = row.try_get("period_id")?;
        let start: i64 = row.try_get("period_start")?;
        let end: i64 = row.try_get("period_end")?;
        let mut metadata = BTreeMap::from([
            ("billing_period_id".into(), period),
            ("period_start_ms".into(), start.to_string()),
            ("period_end_ms".into(), end.to_string()),
        ]);
        if let Some(sub) = row.try_get::<Option<String>, _>("provider_subscription_id")? {
            metadata.insert("provider_subscription_id".into(), sub);
        }
        let event = MeterEvent {
            account_id: owner.into(),
            event_name: "zuse_cloud_overage_cent".into(),
            units: row.try_get("amount_cents")?,
            idempotency_key: row.try_get("idempotency_key")?,
            occurred_at_ms: row
                .try_get::<Option<i64>, _>("occurred_at")?
                .unwrap_or(row.try_get("created_at")?),
            metadata,
        };
        let result = if row.try_get::<String, _>("period_owner")? != owner
            || row.try_get::<String, _>("billing_provider")? != provider
            || row.try_get::<String, _>("currency")? != "USD"
            || start < 0
            || end <= start
            || end > 9_007_199_254_740_991
            || event.occurred_at_ms < start
            || event.occurred_at_ms >= end
        {
            Err(reconciliation())
        } else {
            self.send_meter(&mut tx, &provider, &event, now).await
        };
        match &result {
            Ok(()) => {
                sqlx::query("UPDATE api_cloud_billing_outbox SET acknowledged_at=$2,last_error=NULL WHERE outbox_id=$1").bind(id).bind(now).execute(&mut *tx).await?;
            }
            Err(e) => {
                sqlx::query("UPDATE api_cloud_billing_outbox SET attempt_count=attempt_count+1,next_attempt_at=$2+LEAST(900000,(1::bigint<<LEAST(attempt_count,4)::integer)*60000),last_error=$3 WHERE outbox_id=$1").bind(id).bind(now).bind(e.code).execute(&mut *tx).await?;
            }
        }
        tx.commit().await?;
        result.map(|_| true)
    }
    async fn telemetry_export(&self, id: &str, owner: &str, now: i64) -> Result<bool, ApiError> {
        let mut tx = self.export_transaction(owner).await?;
        let row:Option<Value>=sqlx::query_scalar("SELECT payload FROM api_cloud_usage_outbox WHERE event_id=$1 AND payload->>'accountId'=$2 AND acknowledged_at IS NULL AND next_attempt_at<=$3 FOR UPDATE SKIP LOCKED").bind(id).bind(owner).bind(now).fetch_optional(&mut *tx).await?;
        let Some(v) = row else {
            tx.commit().await?;
            return Ok(false);
        };
        let result = async {
            let event = MeterEvent {
                account_id: owner.into(),
                event_name: v["eventName"].as_str().ok_or_else(invalid)?.into(),
                units: v["units"].as_i64().ok_or_else(invalid)?,
                idempotency_key: v["eventId"].as_str().ok_or_else(invalid)?.into(),
                occurred_at_ms: v["occurredAtMs"].as_i64().ok_or_else(invalid)?,
                metadata: serde_json::from_value(v["metadata"].clone()).map_err(|_| invalid())?,
            };
            if event.event_name == "zuse_cloud_overage_cent" || event.idempotency_key != id {
                return Err(invalid());
            }
            let provider = v["billingProvider"].as_str().ok_or_else(invalid)?;
            self.send_meter(&mut tx, provider, &event, now).await
        }
        .await;
        match &result {
            Ok(()) => {
                sqlx::query("UPDATE api_cloud_usage_outbox SET acknowledged_at=$2,last_error=NULL WHERE event_id=$1").bind(id).bind(now).execute(&mut *tx).await?;
            }
            Err(e) => {
                sqlx::query("UPDATE api_cloud_usage_outbox SET attempt_count=attempt_count+1,next_attempt_at=$2+LEAST(900000,(1::bigint<<LEAST(attempt_count,4)::integer)*60000),last_error=$3 WHERE event_id=$1").bind(id).bind(now).bind(e.code).execute(&mut *tx).await?;
            }
        }
        tx.commit().await?;
        result.map(|_| true)
    }
    /// Drain only accounts explicitly handed to Rust; existing outbox and provider IDs are retained.
    pub async fn export_once(&self, now: i64, limit: i64) -> Result<ExportReport, ApiError> {
        let Some(config) = &self.meter_exports else {
            return Ok(ExportReport::default());
        };
        if !config.enabled() {
            return Ok(ExportReport::default());
        }
        let mut report = ExportReport {
            enabled: true,
            ..Default::default()
        };
        let accounts: Vec<String> = self.accounts.iter().cloned().collect();
        let limit = limit.clamp(1, 100);
        if config.financial {
            let rows:Vec<(String,String)>=sqlx::query_as("SELECT outbox_id,account_id FROM api_cloud_billing_outbox o WHERE acknowledged_at IS NULL AND next_attempt_at<=$1 AND account_id=ANY($2) AND EXISTS(SELECT 1 FROM zuse_rust_control_owners WHERE resource_kind='billing' AND resource_id=o.account_id AND backend='rust') AND EXISTS(SELECT 1 FROM zuse_rust_control_owners WHERE resource_kind='billing-export' AND resource_id=o.account_id AND backend='rust') ORDER BY created_at,outbox_id LIMIT $3").bind(now).bind(&accounts).bind(limit).fetch_all(&self.pool).await?;
            for (id, owner) in rows {
                match self.financial_export(&id, &owner, now).await {
                    Ok(true) => report.financial += 1,
                    Ok(false) => {}
                    Err(_) => report.retried += 1,
                }
            }
        }
        if config.telemetry {
            let rows:Vec<(String,String)>=sqlx::query_as("SELECT event_id,payload->>'accountId' FROM api_cloud_usage_outbox o WHERE acknowledged_at IS NULL AND next_attempt_at<=$1 AND payload->>'accountId'=ANY($2) AND EXISTS(SELECT 1 FROM zuse_rust_control_owners WHERE resource_kind='billing' AND resource_id=o.payload->>'accountId' AND backend='rust') AND EXISTS(SELECT 1 FROM zuse_rust_control_owners WHERE resource_kind='billing-export' AND resource_id=o.payload->>'accountId' AND backend='rust') ORDER BY next_attempt_at,event_id LIMIT $3").bind(now).bind(&accounts).bind(limit).fetch_all(&self.pool).await?;
            for (id, owner) in rows {
                match self.telemetry_export(&id, &owner, now).await {
                    Ok(true) => report.telemetry += 1,
                    Ok(false) => {}
                    Err(_) => report.retried += 1,
                }
            }
        }
        report.reconciled = self.reconcile_meters(now, limit, config, &accounts).await?;
        Ok(report)
    }
    async fn reconcile_meters(
        &self,
        now: i64,
        limit: i64,
        config: &MeterExportConfig,
        accounts: &[String],
    ) -> Result<usize, ApiError> {
        let pending=sqlx::query("SELECT p.period_id,p.account_id,o.provider,p.period_start,p.period_end,SUM(o.amount_cents)::bigint AS expected_units FROM api_cloud_billing_periods p JOIN api_cloud_billing_outbox o ON o.period_id=p.period_id AND o.acknowledged_at IS NOT NULL LEFT JOIN api_cloud_billing_meter_reconciliation_attempts a ON a.period_id=p.period_id AND a.provider=o.provider LEFT JOIN LATERAL(SELECT created_at,observed_units FROM api_cloud_billing_meter_reconciliations WHERE period_id=p.period_id AND provider=o.provider ORDER BY created_at DESC LIMIT 1) r ON TRUE WHERE p.account_id=ANY($2) AND EXISTS(SELECT 1 FROM zuse_rust_control_owners WHERE resource_kind='billing' AND resource_id=p.account_id AND backend='rust') AND EXISTS(SELECT 1 FROM zuse_rust_control_owners WHERE resource_kind='billing-export' AND resource_id=p.account_id AND backend='rust') AND COALESCE(a.attempted_at,0)<=$1-300000 AND o.acknowledged_at<=$1-300000 AND NOT EXISTS(SELECT 1 FROM api_cloud_billing_outbox pending WHERE pending.period_id=p.period_id AND pending.provider=o.provider AND(pending.acknowledged_at IS NULL OR pending.acknowledged_at>$1-300000)) GROUP BY p.period_id,p.account_id,o.provider,p.period_start,p.period_end HAVING COALESCE(MAX(r.created_at),0)<MAX(o.acknowledged_at) OR(MAX(r.observed_units) IS DISTINCT FROM SUM(o.amount_cents) AND COALESCE(MAX(r.created_at),0)<=$1-300000) ORDER BY COALESCE(MAX(a.attempted_at),MAX(r.created_at),0),MAX(o.acknowledged_at),p.period_id,o.provider LIMIT $3").bind(now).bind(accounts).bind(limit).fetch_all(&self.pool).await?;
        let mut count = 0;
        for row in pending {
            let owner: String = row.try_get("account_id")?;
            let provider_id: String = row.try_get("provider")?;
            let period: String = row.try_get("period_id")?;
            let Ok(mut tx) = self.export_transaction(&owner).await else {
                continue;
            };
            // Recheck scheduling under a row lock, so competing workers do not starve other periods.
            let admitted:Option<String>=sqlx::query_scalar("INSERT INTO api_cloud_billing_meter_reconciliation_attempts(period_id,provider,attempted_at) VALUES($1,$2,$3) ON CONFLICT(period_id,provider) DO UPDATE SET attempted_at=EXCLUDED.attempted_at WHERE api_cloud_billing_meter_reconciliation_attempts.attempted_at<=EXCLUDED.attempted_at-300000 RETURNING period_id").bind(&period).bind(&provider_id).bind(now).fetch_optional(&mut *tx).await?;
            if admitted.is_none() {
                tx.commit().await?;
                continue;
            }
            let observed = async {
                let meter = config.meters.get(&provider_id).ok_or_else(reconciliation)?;
                let provider = self
                    .payments
                    .as_ref()
                    .and_then(|p| p.providers.get(&provider_id))
                    .ok_or_else(reconciliation)?;
                let customer: Option<String> = if provider_id == "stripe" {
                    sqlx::query_scalar(
                        "SELECT customer_id FROM api_stripe_customers WHERE account_id=$1",
                    )
                    .bind(&owner)
                    .fetch_optional(&mut *tx)
                    .await?
                    .flatten()
                } else {
                    None
                };
                provider
                    .meter_total(
                        &owner,
                        customer.as_deref(),
                        meter,
                        row.try_get("period_start")?,
                        row.try_get("period_end")?,
                    )
                    .await
            }
            .await;
            if let Ok(observed) = observed {
                let expected: i64 = row.try_get("expected_units")?;
                if expected.unsigned_abs() <= 9_007_199_254_740_991
                    && observed.unsigned_abs() <= 9_007_199_254_740_991
                {
                    sqlx::query("INSERT INTO api_cloud_billing_meter_reconciliations(reconciliation_id,period_id,provider,expected_units,observed_units,created_at) VALUES($1,$2,$3,$4,$5,$6) ON CONFLICT DO NOTHING").bind(format!("meter-reconciliation:{period}:{now}")).bind(&period).bind(&provider_id).bind(expected).bind(observed).bind(now).execute(&mut *tx).await?;
                    count += 1;
                }
            }
            tx.commit().await?;
        }
        Ok(count)
    }
}
