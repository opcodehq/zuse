//! Original checkout contracts, durable Stripe customer reservations and signed receipts.
use super::*;
use crate::tickets::TicketSigner;
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Row};
pub(super) const CLOUD_OFFER: &str = "cloud-workspace-standard-v1";
pub(super) struct CheckoutSettings {
    pub(super) signer: TicketSigner,
}
pub(super) struct ProviderCheckout {
    pub offers: HashMap<String, String>,
    pub overage: Option<String>,
    pub automatic_tax: bool,
}
#[derive(Clone, Debug)]
pub struct CustomerPage {
    pub matches: Vec<String>,
    pub next_cursor: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckoutSummary {
    pub checkout_id: String,
    pub amount_cents: i64,
    pub currency: String,
    pub created_at_ms: i64,
    pub status: String,
}
pub(super) fn required<'a>(v: &'a Value, key: &str) -> Result<&'a str, ApiError> {
    v[key]
        .as_str()
        .filter(|v| !v.is_empty() && v.len() <= 16384)
        .ok_or_else(unavailable)
}
impl ProviderCheckout {
    pub(super) fn from_env(prefix: &str, kind: &str) -> anyhow::Result<Option<Self>> {
        let mut offers = HashMap::new();
        let mut ids = std::collections::HashSet::new();
        for (suffix, offer) in [
            ("CLOUD_WORKSPACE_STANDARD_V1", CLOUD_OFFER),
            ("PERSISTENT_STANDARD_V1", "persistent-standard-v1"),
        ] {
            let value = std::env::var(format!("{prefix}_{kind}_{suffix}"))
                .ok()
                .or_else(|| {
                    if prefix == "POLAR" && offer == CLOUD_OFFER {
                        std::env::var("POLAR_PRODUCT_SANDBOX_STANDARD_V1").ok()
                    } else {
                        None
                    }
                });
            if let Some(value) = value.filter(|v| !v.trim().is_empty()) {
                anyhow::ensure!(
                    ids.insert(value.clone()),
                    "Duplicate checkout offer identifier"
                );
                offers.insert(offer.into(), value);
            }
        }
        if offers.is_empty() {
            return Ok(None);
        }
        let mode = std::env::var("STRIPE_CLOUD_BILLING_MODE").unwrap_or_else(|_| "metered".into());
        anyhow::ensure!(
            prefix != "STRIPE" || matches!(mode.as_str(), "metered" | "subscription-only"),
            "Invalid Stripe cloud billing mode"
        );
        let overage = if prefix == "STRIPE" && mode == "metered" && offers.contains_key(CLOUD_OFFER)
        {
            let price = std::env::var("STRIPE_CLOUD_OVERAGE_PRICE_ID")
                .ok()
                .filter(|v| !v.trim().is_empty());
            let meter = std::env::var("STRIPE_CLOUD_OVERAGE_METER_ID")
                .ok()
                .filter(|v| !v.trim().is_empty());
            // Incomplete metering configuration cannot sell a metered cloud subscription.
            if price.is_none() || meter.is_none() {
                offers.remove(CLOUD_OFFER);
                None
            } else {
                price
            }
        } else {
            None
        };
        anyhow::ensure!(
            overage.as_ref().is_none_or(|id| !ids.contains(id)),
            "Overage price cannot be a base price"
        );
        Ok(Some(Self {
            offers,
            overage,
            automatic_tax: std::env::var("STRIPE_AUTOMATIC_TAX_ENABLED").as_deref() != Ok("false"),
        }))
    }
}
impl CheckoutSummary {
    fn validate(self) -> Result<Self, ApiError> {
        if self.amount_cents < 0
            || self.amount_cents > 9_007_199_254_740_991
            || self.created_at_ms < 0
            || self.created_at_ms > 9_007_199_254_740_991
            || self.currency.len() != 3
            || !self.currency.bytes().all(|v| v.is_ascii_alphabetic())
        {
            return Err(unavailable());
        }
        Ok(self)
    }
    pub(super) fn stripe(v: &Value) -> Result<Self, ApiError> {
        Self {
            checkout_id: required(v, "id")?.into(),
            amount_cents: v["amount_total"].as_i64().unwrap_or(0),
            currency: v["currency"].as_str().unwrap_or("usd").into(),
            created_at_ms: v["created"]
                .as_i64()
                .and_then(|v| v.checked_mul(1000))
                .ok_or_else(unavailable)?,
            status: if matches!(
                v["payment_status"].as_str(),
                Some("paid" | "no_payment_required")
            ) {
                "paid"
            } else if v["status"] == "expired" {
                "failed"
            } else {
                "pending"
            }
            .into(),
        }
        .validate()
    }
    pub(super) fn polar(v: &Value) -> Result<Self, ApiError> {
        Self {
            checkout_id: required(v, "id")?.into(),
            amount_cents: v["total_amount"].as_i64().ok_or_else(unavailable)?,
            currency: required(v, "currency")?.into(),
            created_at_ms: chrono::DateTime::parse_from_rfc3339(required(v, "created_at")?)
                .map_err(|_| unavailable())?
                .timestamp_millis(),
            status: match v["status"].as_str() {
                Some("succeeded") => "paid",
                Some("open" | "confirmed") => "pending",
                _ => "failed",
            }
            .into(),
        }
        .validate()
    }
}
impl PaymentProviders {
    pub fn with_checkout(mut self, signer: TicketSigner) -> Self {
        self.checkout = Some(Arc::new(CheckoutSettings { signer }));
        self
    }
}
fn reconcile() -> ApiError {
    ApiError::conflict("billing_customer_reconciliation_required")
}
pub(super) async fn stripe_customer(
    pool: &PgPool,
    provider: &dyn PortalProvider,
    owner: &str,
    now: i64,
) -> Result<String, ApiError> {
    // Commit reservation before remote creation. Losing its reply retains the original retry key.
    sqlx::query("INSERT INTO api_stripe_customers(account_id,created_at) VALUES($1,$2) ON CONFLICT DO NOTHING").bind(owner).bind(now).execute(pool).await?;
    let mut row:Value=sqlx::query_scalar("SELECT to_jsonb(c)||jsonb_build_object('reserved_at',COALESCE(reservation_created_at,created_at)) FROM api_stripe_customers c WHERE account_id=$1").bind(owner).fetch_one(pool).await?;
    if let Some(id) = row["customer_id"].as_str() {
        provider.verify_customer(owner, id).await?;
        return Ok(id.into());
    }
    let mut generation = row["generation"].as_i64().ok_or_else(unavailable)?;
    if generation < 0 {
        return Err(reconcile());
    }
    let created = row["reserved_at"].as_i64().ok_or_else(unavailable)?;
    if now - created >= 23 * 3600000 {
        if row["recovery_complete"] != true {
            let cursor = row["recovery_cursor"].as_str();
            let page = provider.customer_page(owner, cursor).await?;
            if page.next_cursor.as_deref() == cursor && cursor.is_some() {
                return Err(reconcile());
            }
            let mut matches: Vec<String> = serde_json::from_value(row["recovery_matches"].clone())
                .map_err(|_| unavailable())?;
            for id in page.matches {
                if !matches.contains(&id) {
                    matches.push(id);
                }
                if matches.len() >= 2 {
                    break;
                }
            }
            let complete = page.next_cursor.is_none() || matches.len() > 1;
            let updated:Option<Value>=sqlx::query_scalar("UPDATE api_stripe_customers SET recovery_cursor=$3,recovery_matches=$4,recovery_complete=$5,recovery_lease_until=0,recovery_attempted_at=$6 WHERE account_id=$1 AND generation=$2 AND customer_id IS NULL AND NOT recovery_complete AND recovery_cursor IS NOT DISTINCT FROM $7 RETURNING to_jsonb(api_stripe_customers)").bind(owner).bind(generation).bind(&page.next_cursor).bind(json!(matches)).bind(complete).bind(now).bind(cursor).fetch_optional(pool).await?;
            row = updated.ok_or_else(reconcile)?;
            if !complete {
                return Err(reconcile());
            }
        }
        let matches: Vec<String> =
            serde_json::from_value(row["recovery_matches"].clone()).map_err(|_| unavailable())?;
        if matches.len() > 1 {
            return Err(reconcile());
        }
        if let Some(id) = matches.first() {
            provider.verify_customer(owner, id).await?;
            link_customer(pool, owner, id).await?;
            return Ok(id.clone());
        }
        let renewed:Option<i64>=sqlx::query_scalar("UPDATE api_stripe_customers SET generation=generation+1,reservation_created_at=$3,recovery_cursor=NULL,recovery_matches='[]'::jsonb,recovery_complete=FALSE,recovery_attempted_at=NULL,recovery_lease_until=0 WHERE account_id=$1 AND generation=$2 AND customer_id IS NULL AND recovery_complete AND recovery_matches='[]'::jsonb RETURNING generation::bigint").bind(owner).bind(generation).bind(now).fetch_optional(pool).await?;
        generation = renewed.ok_or_else(reconcile)?;
    }
    let key = if generation == 0 {
        format!("zuse-customer:{owner}")
    } else {
        format!("zuse-customer:{owner}:generation:{generation}")
    };
    let id = provider.create_customer(owner, &key).await?;
    provider.verify_customer(owner, &id).await?;
    link_customer(pool, owner, &id).await?;
    Ok(id)
}
async fn link_customer(pool: &PgPool, owner: &str, id: &str) -> Result<(), ApiError> {
    let owner:Option<String>=sqlx::query_scalar("UPDATE api_stripe_customers SET customer_id=$2 WHERE account_id=$1 AND (customer_id IS NULL OR customer_id=$2) RETURNING account_id").bind(owner).bind(id).fetch_optional(pool).await?;
    if owner.is_none() {
        return Err(reconcile());
    }
    Ok(())
}
impl BillingControl {
    async fn checkout(
        &self,
        owner: &str,
        offer: &str,
        now: i64,
        key: Option<&str>,
    ) -> Result<String, ApiError> {
        if !matches!(offer, CLOUD_OFFER | "persistent-standard-v1") {
            return Err(ApiError::bad_request("invalid_machine_offer"));
        }
        if owner.starts_with("organization:") && offer != CLOUD_OFFER {
            return Err(ApiError::forbidden("workspace_scope_not_supported"));
        }
        if let Some(url) = self.native_checkout(owner, offer, now, key).await? {
            return Ok(url);
        }
        let providers = self.payments.as_ref().ok_or_else(unavailable)?;
        let settings = providers
            .checkout
            .as_ref()
            .ok_or_else(|| ApiError::unavailable("billing_approval_pending"))?;
        if offer != CLOUD_OFFER {
            return Err(ApiError::unavailable(
                "billing_machine_processing_unavailable",
            ));
        }
        let provider = providers
            .providers
            .get(&providers.default)
            .ok_or_else(unavailable)?;
        if !provider.supports_checkout(offer) {
            return Err(ApiError::unavailable("billing_approval_pending"));
        }
        let mut tx = self.owned_transaction(owner).await?;
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,68130))")
            .bind(owner)
            .execute(&mut *tx)
            .await?;
        let existing=sqlx::query("SELECT entitlement_id,provider,provider_subscription_id,status,paid_through FROM api_entitlements WHERE account_id=$1 AND offer_id=$2 AND status<>'ended' FOR UPDATE").bind(owner).bind(offer).fetch_all(&mut *tx).await?;
        let mut subscribed = false;
        for row in existing {
            let id: Option<String> = row.try_get("provider_subscription_id")?;
            let (status, through) = if let Some(id) = id {
                let original_provider: String = row.try_get("provider")?;
                let adapter = providers
                    .providers
                    .get(&original_provider)
                    .ok_or_else(unavailable)?;
                let sub = adapter.subscription(&id).await?;
                if sub.owner != owner || sub.id != id || sub.offer != offer {
                    return Err(ApiError::unavailable("billing_subscription_mismatch"));
                }
                if original_provider == "stripe" {
                    let customer: Option<String> = sqlx::query_scalar(
                        "SELECT customer_id FROM api_stripe_customers WHERE account_id=$1",
                    )
                    .bind(owner)
                    .fetch_optional(&mut *tx)
                    .await?
                    .flatten();
                    if customer.is_none() || customer != sub.customer {
                        return Err(unavailable());
                    }
                }
                let entitlement: String = row.try_get("entitlement_id")?;
                sqlx::query("UPDATE api_entitlements SET status=$2,period_start=$3,paid_through=$4,ended_at=CASE WHEN $2='ended' AND $4<=$5 THEN COALESCE(ended_at,$5) ELSE NULL END,updated_at=$5 WHERE entitlement_id=$1").bind(entitlement).bind(&sub.status).bind(sub.period_start).bind(sub.paid_through).bind(now).execute(&mut *tx).await?;
                (sub.status, Some(sub.paid_through))
            } else {
                (
                    row.try_get::<String, _>("status")?,
                    row.try_get::<Option<i64>, _>("paid_through")?,
                )
            };
            if matches!(status.as_str(), "pending" | "active" | "grace")
                && through.is_none_or(|end| end > now)
            {
                subscribed = true;
            }
        }
        if subscribed {
            tx.commit().await?;
            return Err(ApiError::conflict("machine_subscription_exists"));
        }
        let customer = if provider.provider_id() == "stripe" {
            Some(stripe_customer(&self.effects_pool, provider.as_ref(), owner, now).await?)
        } else {
            None
        };
        let ticket = settings.signer.checkout(owner, offer, now)?;
        let mut success = Url::parse(&format!(
            "{}/v1/billing/checkout/complete",
            settings.signer.issuer()
        ))
        .map_err(|_| unavailable())?;
        success
            .query_pairs_mut()
            .append_pair("offer", offer)
            .append_pair("t", &ticket);
        let success = format!("{success}&checkout_id={{CHECKOUT_ID}}");
        let url = provider
            .checkout(owner, customer.as_deref(), offer, &success)
            .await?;
        secure_url(&url)?;
        tx.commit().await?;
        Ok(url)
    }
}
impl Store {
    pub async fn billing_checkout(
        &self,
        owner: &str,
        offer: &str,
        now: i64,
    ) -> Result<String, ApiError> {
        self.billing_checkout_with_key(owner, offer, now, None)
            .await
    }
    pub async fn billing_checkout_with_key(
        &self,
        owner: &str,
        offer: &str,
        now: i64,
        key: Option<&str>,
    ) -> Result<String, ApiError> {
        self.billing
            .as_ref()
            .ok_or_else(|| ApiError::unavailable("cloud_billing_writes_disabled"))?
            .checkout(owner, offer, now, key)
            .await
    }
    pub async fn checkout_receipt(
        &self,
        ticket: Option<&str>,
        checkout: Option<&str>,
        now: i64,
    ) -> Option<CheckoutSummary> {
        let billing = self.billing.as_ref()?;
        let signer = billing
            .native
            .as_ref()
            .and_then(|n| n.signer.as_ref())
            .or_else(|| {
                billing
                    .payments
                    .as_ref()?
                    .checkout
                    .as_ref()
                    .map(|s| &s.signer)
            })?;
        let claims = signer.verify_checkout(ticket?, now).ok()?;
        let owner = claims["sub"].as_str()?;
        let id = checkout?.trim();
        if id.is_empty() || id.len() > 1024 || id.contains('{') {
            return None;
        }
        if id.starts_with("payment:") {
            return self.billing.as_ref()?.native_receipt(owner, id).await;
        }
        let providers = billing.payments.as_ref()?;
        // Unauthenticated browser receipt lookup requires the signed account ticket.
        tokio::time::timeout(Duration::from_secs(2), async {
            for provider in std::iter::once(&providers.default).chain(
                providers
                    .providers
                    .keys()
                    .filter(|id| *id != &providers.default),
            ) {
                if let Ok(Some(summary)) = providers
                    .providers
                    .get(provider)?
                    .checkout_summary(owner, id)
                    .await
                {
                    return Some(summary);
                }
            }
            None
        })
        .await
        .ok()
        .flatten()
    }
}

#[cfg(test)]
mod tests;
