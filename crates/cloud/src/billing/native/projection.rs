use super::*;
use axum::http::HeaderMap;
use sha2::{Digest, Sha256};

impl BillingControl {
    pub(super) async fn sync_native_customer(&self, owner: &str) -> Result<(), ApiError> {
        let native = self
            .native
            .as_ref()
            .ok_or_else(|| ApiError::unavailable("embedded_billing_disabled"))?;
        let customer = native
            .engine
            .call("customers.get", json!({"customerId":owner}))
            .await
            .map_err(failure)?;
        if let Some(id) = customer["stripeCustomerId"].as_str() {
            let mut tx = self.owned_transaction(owner).await?;
            let linked: Option<String> = sqlx::query_scalar("INSERT INTO api_stripe_customers(account_id,customer_id,created_at) VALUES($1,$2,$3) ON CONFLICT(account_id) DO UPDATE SET customer_id=EXCLUDED.customer_id WHERE api_stripe_customers.customer_id IS NULL OR api_stripe_customers.customer_id=EXCLUDED.customer_id RETURNING account_id").bind(owner).bind(id).bind(chrono::Utc::now().timestamp_millis()).fetch_optional(&mut *tx).await?;
            if linked.is_none() {
                return Err(ApiError::conflict(
                    "billing_customer_reconciliation_required",
                ));
            }
            sqlx::query("UPDATE zuse_brokeboy_accounts SET stripe_customer_id=$2 WHERE account_id=$1 AND (stripe_customer_id IS NULL OR stripe_customer_id=$2)").bind(owner).bind(id).execute(&mut *tx).await?;
            tx.commit().await?;
        }
        Ok(())
    }
    pub(super) async fn sync_native_subscriptions(&self, owner: &str) -> Result<(), ApiError> {
        let native = self
            .native
            .as_ref()
            .ok_or_else(|| ApiError::unavailable("embedded_billing_disabled"))?;
        self.sync_native_customer(owner).await?;
        let mut tx = self.owned_transaction(owner).await?;
        // Read canonical engine state while serializing host projections, so concurrent
        // notifications and cap edits cannot rewind a previously projected subscription.
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,68130))")
            .bind(owner)
            .execute(&mut *tx)
            .await?;
        let subscriptions = native
            .engine
            .call("subscriptions.list", json!({"customerId":owner}))
            .await
            .map_err(failure)?;
        let now = chrono::Utc::now().timestamp_millis();
        for value in subscriptions
            .as_array()
            .ok_or_else(|| ApiError::unavailable("invalid_billing_result"))?
        {
            if value["plan"]["id"] != CLOUD_PLAN {
                continue;
            }
            native.validate_compute_plan(&value["plan"])?;
            let provider = value["provider"].as_str().unwrap_or("stripe");
            let sub = project(owner, value)?;
            if sub.paid_through <= sub.period_start {
                // An unpaid initial invoice grants no period; a lost paid renewal blocks admission.
                sqlx::query("UPDATE api_entitlements SET status=$3,paid_through=$4,updated_at=$5 WHERE account_id=$1 AND provider_subscription_id=$2").bind(owner).bind(&sub.id).bind(&sub.status).bind(sub.paid_through).bind(now).execute(&mut *tx).await?;
                sqlx::query("UPDATE api_cloud_billing_periods SET status='billing-hold',updated_at=$3 WHERE account_id=$1 AND provider_subscription_id=$2").bind(owner).bind(&sub.id).bind(now).execute(&mut *tx).await?;
                continue;
            }
            let event = format!(
                "native:{:x}",
                Sha256::digest(
                    serde_json::to_vec(value)
                        .map_err(|_| ApiError::unavailable("invalid_billing_result"))?
                )
            );
            self.project_subscription(&mut tx, provider, &event, &sub, now, false)
                .await?;
            sqlx::query("UPDATE api_cloud_billing_periods SET shared_credit_account=true,included_provider_cost_micros=40000000 WHERE account_id=$1 AND provider_subscription_id=$2 AND period_start=$3").bind(owner).bind(&sub.id).bind(sub.period_start).execute(&mut *tx).await?;
        }
        tx.commit().await?;
        self.sync_credit_capacity(owner).await?;
        Ok(())
    }
    pub(in crate::billing) async fn native_webhook(
        &self,
        provider: Option<&str>,
        headers: &HeaderMap,
        body: &[u8],
    ) -> Result<Option<Value>, ApiError> {
        let Some(native) = &self.native else {
            return Ok(None);
        };
        let stripe = headers.contains_key("stripe-signature");
        let polar = headers.contains_key("webhook-signature");
        let detected = match (stripe, polar) {
            (true, false) => "stripe",
            (false, true) => "polar",
            _ => return Err(ApiError::unauthorized("invalid_billing_event")),
        };
        if provider.is_some_and(|p| p != detected) {
            return Err(ApiError::unauthorized("invalid_billing_event"));
        }
        let original_headers = headers;
        let headers: std::collections::BTreeMap<String, String> = headers
            .iter()
            .filter_map(|(k, v)| v.to_str().ok().map(|v| (k.as_str().into(), v.into())))
            .collect();
        let Some(event) = native
            .provider
            .verify(headers.clone(), body.to_vec())
            .await
            .map_err(failure)?
        else {
            return Ok(None);
        };
        // Preserve historical invoice-credit purchase provenance in its original handler.
        let legacy_credit = event.data["object"]["metadata"]["purpose"]
            .as_str()
            .is_some_and(|p| p.starts_with("zuse"));
        if legacy_credit {
            return Ok(None);
        }
        // Refund/dispute objects often omit purchase metadata. Confirm historical
        // provenance through the original canonical checkout lookup before choosing
        // a financial writer; enrollment does not transfer old purchase allocations.
        if detected == "stripe"
            && (event.kind.starts_with("charge.") || event.kind.starts_with("refund."))
        {
            if let Some(legacy) = self
                .payments
                .as_ref()
                .and_then(|p| p.providers.get("stripe"))
            {
                if let Some(crate::billing::PaymentEvent::Prepaid(credit)) = legacy.verify_event(
                    original_headers,
                    body,
                    chrono::Utc::now().timestamp_millis(),
                )? {
                    if legacy.credit_purchase(&credit).await?.is_some() {
                        return Ok(None);
                    }
                }
            }
        }
        let owner = if !event.customer_id.is_empty() {
            Some(event.customer_id.clone())
        } else if let Some(customer) = event.data["providerCustomerId"].as_str() {
            sqlx::query_scalar(
                "SELECT account_id FROM zuse_brokeboy_accounts WHERE stripe_customer_id=$1",
            )
            .bind(customer)
            .fetch_optional(&self.pool)
            .await?
        } else {
            None
        };
        let Some(owner) = owner else {
            return Ok(None);
        };
        if !native.enrolled(&self.pool, &owner).await? {
            return Ok(None);
        }
        let tx = self.owned_transaction(&owner).await?;
        let accepted = native
            .engine
            .webhook(headers, body.to_vec())
            .await
            .map_err(failure)?;
        tx.commit().await?;
        // Durable acceptance is sufficient for a 2xx; the billing worker owns retries
        // and projection even if this HTTP request disappears immediately afterwards.
        Ok(Some(accepted))
    }
}
fn project(owner: &str, value: &Value) -> Result<crate::billing::Subscription, ApiError> {
    let status = match text(value, "status")? {
        "active" | "trialing" => "active",
        "past_due" | "paused" => "grace",
        "pending" => "pending",
        "ended" => "ended",
        _ => return Err(ApiError::unavailable("invalid_billing_result")),
    };
    Ok(crate::billing::Subscription {
        owner: owner.into(),
        id: text(value, "id")?.into(),
        offer: text(&value["plan"], "id")?.into(),
        status: status.into(),
        period_start: value["periodStart"]
            .as_i64()
            .filter(|v| *v >= 0)
            .ok_or_else(|| ApiError::unavailable("invalid_billing_result"))?,
        paid_through: value["paidThrough"]
            .as_i64()
            .filter(|v| *v >= 0)
            .ok_or_else(|| ApiError::unavailable("invalid_billing_result"))?,
        customer: None,
    })
}
