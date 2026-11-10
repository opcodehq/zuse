//! Original Stripe meter contracts. Only canonical billable periods admit Stripe charges.
use super::*;
use crate::billing::MeterEvent;
use serde::Serialize;
use sha2::{Digest, Sha256};
fn reconcile() -> ApiError {
    ApiError::conflict("billing_meter_reconciliation_required")
}
pub(super) fn whole(v: &Value) -> Result<i64, ApiError> {
    if let Some(n) = v.as_i64() {
        if n.unsigned_abs() <= 9_007_199_254_740_991 {
            return Ok(n);
        }
    }
    if let Some(n) = v.as_f64() {
        if n.is_finite() && n.fract() == 0. && n.abs() <= 9_007_199_254_740_991.0 {
            return Ok(n as i64);
        }
    }
    Err(reconcile())
}
#[derive(Serialize)]
pub(super) struct StripeCommand<'a> {
    event_name: &'a str,
    identifier: String,
    timestamp: i64,
    payload: StripePayload<'a>,
}
#[derive(Serialize)]
struct StripePayload<'a> {
    stripe_customer_id: &'a str,
    value: String,
}
impl<'a> StripeCommand<'a> {
    pub(super) fn new(
        event: &'a MeterEvent,
        customer: &'a str,
        now: i64,
    ) -> Result<Self, ApiError> {
        event.validate()?;
        let timestamp = event.occurred_at_ms / 1000;
        if timestamp * 1000 < now - 35 * 86400000 || timestamp * 1000 > now + 300000 {
            return Err(reconcile());
        }
        Ok(Self {
            event_name: &event.event_name,
            identifier: format!(
                "zuse_{:x}",
                Sha256::digest(event.idempotency_key.as_bytes())
            ),
            timestamp,
            payload: StripePayload {
                stripe_customer_id: customer,
                value: event.units.to_string(),
            },
        })
    }
    pub(super) fn key(&self) -> &str {
        &self.identifier
    }
}
impl StripePortal {
    async fn pages(
        &self,
        path: &[&str],
        query: &[(String, String)],
    ) -> Result<Vec<Value>, ApiError> {
        let mut result = vec![];
        let mut cursor: Option<String> = None;
        loop {
            let mut request = self.http.request(reqwest::Method::GET, path)?.query(query);
            if let Some(cursor) = &cursor {
                request = request.query(&[("starting_after", cursor)]);
            }
            let v = ProviderHttp::json(request).await?;
            let data = v["data"].as_array().ok_or_else(unavailable)?;
            let more = v["has_more"].as_bool().ok_or_else(unavailable)?;
            let next = if more {
                Some(checkout::required(data.last().ok_or_else(unavailable)?, "id")?.to_owned())
            } else {
                None
            };
            if more && next == cursor {
                return Err(reconcile());
            }
            result.extend(data.iter().cloned());
            if result.len() > 10000 {
                return Err(reconcile());
            }
            if !more {
                return Ok(result);
            }
            cursor = next;
        }
    }
    async fn can_report(&self, customer: &str, event: &MeterEvent) -> Result<bool, ApiError> {
        let Some(price) = self.meter_price.as_deref() else {
            return Ok(false);
        };
        let subscription = event
            .metadata
            .get("provider_subscription_id")
            .filter(|v| !v.is_empty())
            .ok_or_else(reconcile)?;
        let start = event
            .metadata
            .get("period_start_ms")
            .and_then(|v| v.parse::<i64>().ok())
            .ok_or_else(reconcile)?;
        let end = event
            .metadata
            .get("period_end_ms")
            .and_then(|v| v.parse::<i64>().ok())
            .ok_or_else(reconcile)?;
        if start < 0 || end <= start || end > 9_007_199_254_740_991 {
            return Err(reconcile());
        }
        let sub = ProviderHttp::json(
            self.http
                .request(reqwest::Method::GET, &["subscriptions", subscription])?,
        )
        .await?;
        let sub_customer = sub["customer"]
            .as_str()
            .or_else(|| sub["customer"]["id"].as_str());
        if sub["id"] != subscription.as_str() || sub_customer != Some(customer) {
            return Ok(false);
        }
        let same_period = |a: &Value, b: &Value| {
            a.as_i64().and_then(|v| v.checked_mul(1000)) == Some(start)
                && b.as_i64().and_then(|v| v.checked_mul(1000)) == Some(end)
        };
        if !matches!(
            sub["status"].as_str(),
            Some("canceled" | "incomplete_expired")
        ) && sub["items"]["data"].as_array().is_some_and(|items| {
            items.iter().any(|i| {
                i["price"]["id"] == price
                    && same_period(&i["current_period_start"], &i["current_period_end"])
            })
        }) {
            return Ok(true);
        }
        let invoices = self
            .pages(
                &["invoices"],
                &[
                    ("subscription".into(), subscription.clone()),
                    ("created[gte]".into(), (end / 1000 - 300).to_string()),
                    ("limit".into(), "10".into()),
                ],
            )
            .await?;
        for invoice in invoices {
            let id = checkout::required(&invoice, "id")?;
            let lines = self
                .pages(
                    &["invoices", id, "lines"],
                    &[("limit".into(), "100".into())],
                )
                .await?;
            for line in lines {
                if line["pricing"]["price_details"]["price"] == price
                    && same_period(&line["period"]["start"], &line["period"]["end"])
                {
                    return Ok(invoice["status"] == "draft");
                }
            }
        }
        Ok(false)
    }
    pub(super) async fn report_usage(
        &self,
        event: &MeterEvent,
        customer: &str,
    ) -> Result<(), ApiError> {
        if event.units == 0 {
            return Ok(());
        }
        if event.event_name == "zuse_cloud_overage_cent"
            && !self.can_report(customer, event).await?
        {
            return Err(reconcile());
        }
        let command = StripeCommand::new(event, customer, chrono::Utc::now().timestamp_millis())?;
        ProviderHttp::json(
            self.http
                .request(reqwest::Method::POST, &["billing", "meter_events"])?
                .header("Idempotency-Key", command.key())
                .form(&[
                    ("event_name", command.event_name),
                    ("identifier", command.key()),
                    ("timestamp", &command.timestamp.to_string()),
                    ("payload[stripe_customer_id]", customer),
                    ("payload[value]", command.payload.value.as_str()),
                ]),
        )
        .await?;
        Ok(())
    }
    pub(super) async fn usage_total(
        &self,
        owner: &str,
        customer: &str,
        meter: &str,
        start: i64,
        end: i64,
    ) -> Result<i64, ApiError> {
        if start < 0 || end <= start || end > 9_007_199_254_740_991 {
            return Err(reconcile());
        }
        self.verify_customer(owner, customer).await?;
        let start = ((start + 59999) / 60000) * 60;
        let end = ((end + 59999) / 60000) * 60;
        if start >= end {
            return Err(reconcile());
        }
        let values = self
            .pages(
                &["billing", "meters", meter, "event_summaries"],
                &[
                    ("customer".into(), customer.into()),
                    ("start_time".into(), start.to_string()),
                    ("end_time".into(), end.to_string()),
                    ("limit".into(), "100".into()),
                ],
            )
            .await?;
        let mut total = 0_i64;
        for v in values {
            total = total
                .checked_add(whole(&v["aggregated_value"])?)
                .ok_or_else(reconcile)?;
        }
        if total.unsigned_abs() > 9_007_199_254_740_991 {
            return Err(reconcile());
        }
        Ok(total)
    }
}
#[cfg(test)]
mod tests;
