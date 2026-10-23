//! Stripe invoice credits with original delivery keys and durable financial receipts.
use super::*;
use serde::Serialize;
use sqlx::Row;
use zuse_protocol::cloud_billing::{PREPAID_CREDIT_AMOUNTS, PrepaidBalance};
const PURPOSE: &str = "zuse_prepaid_v1";
fn reconcile() -> ApiError {
    ApiError::conflict("billing_credit_reconciliation_required")
}
fn integer(v: &Value, key: &str) -> Result<i64, ApiError> {
    v[key].as_i64().ok_or_else(reconcile)
}
fn id(v: &Value) -> Result<&str, ApiError> {
    v.as_str()
        .or_else(|| v["id"].as_str())
        .filter(|s| !s.is_empty() && s.len() <= 1024)
        .ok_or_else(reconcile)
}
#[derive(Clone, Debug)]
pub struct CreditDispute {
    pub id: String,
    pub amount: i64,
    pub reinstated: bool,
}
#[derive(Clone, Debug)]
pub struct CreditEvent {
    pub event_id: String,
    pub session_id: Option<String>,
    pub charge_id: Option<String>,
    pub dispute: Option<CreditDispute>,
}
#[derive(Clone, Debug)]
pub struct CreditPurchase {
    pub id: String,
    pub owner: String,
    pub customer: String,
    pub amount: i64,
    pub paid: bool,
    pub currency: String,
    pub price: String,
    pub quantity: i64,
    pub charge: String,
    pub disputed: bool,
    pub refunds: Vec<(String, i64)>,
}
impl CreditEvent {
    pub(super) fn from_verified(event_id: &str, v: &Value) -> Result<Option<Self>, ApiError> {
        let object = &v["data"]["object"];
        let mut event = Self {
            event_id: event_id.into(),
            session_id: None,
            charge_id: None,
            dispute: None,
        };
        match v["type"].as_str() {
            Some("checkout.session.completed" | "checkout.session.async_payment_succeeded") => {
                event.session_id = Some(id(&object["id"])?.into())
            }
            Some("charge.refunded") => event.charge_id = Some(id(&object["id"])?.into()),
            Some("refund.updated") => {
                if object["charge"].is_null() {
                    return Ok(None);
                }
                event.charge_id = Some(id(&object["charge"])?.into());
            }
            Some("charge.dispute.funds_withdrawn" | "charge.dispute.funds_reinstated") => {
                event.charge_id = Some(id(&object["charge"])?.into());
                event.dispute = Some(CreditDispute {
                    id: id(&object["id"])?.into(),
                    amount: integer(object, "amount")?,
                    reinstated: v["type"] == "charge.dispute.funds_reinstated",
                });
            }
            _ => return Ok(None),
        }
        Ok(Some(event))
    }
}
impl StripePortal {
    pub(super) async fn credit_balance(
        &self,
        owner: &str,
        customer: Option<&str>,
    ) -> Result<PrepaidBalance, ApiError> {
        let mut balance = 0_i64;
        if let Some(customer) = customer {
            let v = ProviderHttp::json(
                self.http
                    .request(reqwest::Method::GET, &["customers", customer])?,
            )
            .await?;
            if v["id"] != customer
                || v["deleted"] == true
                || v["metadata"]["account_id"] != owner
                || v["currency"].as_str().is_some_and(|s| s != "usd")
            {
                return Err(unavailable());
            }
            balance = integer(&v, "balance")?;
            if balance.unsigned_abs() > 9_007_199_254_740_991 {
                return Err(reconcile());
            }
        }
        Ok(PrepaidBalance {
            available: self.prepaid_available(),
            credit_cents: if balance < 0 { -balance } else { 0 },
            debit_cents: balance.max(0),
            currency: "usd".into(),
        })
    }
    pub(super) async fn credit_checkout(
        &self,
        owner: &str,
        customer: &str,
        amount: i64,
        success: &str,
    ) -> Result<String, ApiError> {
        let price = self
            .credit_price
            .as_deref()
            .ok_or_else(|| ApiError::unavailable("billing_approval_pending"))?;
        if !PREPAID_CREDIT_AMOUNTS.contains(&amount) {
            return Err(ApiError::bad_request("invalid_prepaid_amount"));
        }
        let v = ProviderHttp::json(
            self.http
                .request(reqwest::Method::GET, &["prices", price])?,
        )
        .await?;
        if v["id"] != price
            || v["active"] != true
            || v["currency"] != "usd"
            || v["unit_amount"] != 100
            || !v["recurring"].is_null()
            || v["billing_scheme"] != "per_unit"
            || !v["transform_quantity"].is_null()
        {
            return Err(reconcile());
        }
        self.verify_customer(owner, customer).await?;
        let quantity = (amount / 100).to_string();
        let value = ProviderHttp::json(
            self.http
                .request(reqwest::Method::POST, &["checkout", "sessions"])?
                .form(&[
                    ("mode", "payment"),
                    ("customer", customer),
                    ("client_reference_id", owner),
                    ("allowed_payment_method_types[0]", "card"),
                    ("success_url", success),
                    ("line_items[0][price]", price),
                    ("line_items[0][quantity]", quantity.as_str()),
                    ("metadata[purpose]", PURPOSE),
                    ("metadata[account_id]", owner),
                    ("payment_intent_data[metadata][purpose]", PURPOSE),
                    ("payment_intent_data[metadata][account_id]", owner),
                ]),
        )
        .await?;
        let url = checkout::required(&value, "url")?;
        secure_url(url)?;
        Ok(url.into())
    }
    pub(super) async fn purchase(
        &self,
        event: &CreditEvent,
    ) -> Result<Option<CreditPurchase>, ApiError> {
        let session = if let Some(session) = &event.session_id {
            session.clone()
        } else {
            let charge = event.charge_id.as_deref().ok_or_else(reconcile)?;
            let value = ProviderHttp::json(
                self.http
                    .request(reqwest::Method::GET, &["charges", charge])?,
            )
            .await?;
            if value["id"] != charge {
                return Err(reconcile());
            }
            if value["payment_intent"].is_null() {
                return Ok(None);
            }
            let intent = id(&value["payment_intent"])?;
            let sessions = ProviderHttp::json(
                self.http
                    .request(reqwest::Method::GET, &["checkout", "sessions"])?
                    .query(&[("payment_intent", intent), ("limit", "2")]),
            )
            .await?;
            let data = sessions["data"].as_array().ok_or_else(reconcile)?;
            if data.len() != 1 || sessions["has_more"] == true {
                return Ok(None);
            }
            id(&data[0]["id"])?.into()
        };
        let v = ProviderHttp::json(
            self.http
                .request(reqwest::Method::GET, &["checkout", "sessions", &session])?
                .query(&[
                    ("expand[]", "payment_intent.latest_charge"),
                    ("expand[]", "line_items"),
                ]),
        )
        .await?;
        if v["id"] != session {
            return Err(reconcile());
        }
        if v["mode"] != "payment" || v["metadata"]["purpose"] != PURPOSE {
            return Ok(None);
        }
        let intent = &v["payment_intent"];
        let charge = &intent["latest_charge"];
        let items = v["line_items"]["data"].as_array().ok_or_else(reconcile)?;
        if !intent.is_object()
            || !charge.is_object()
            || items.len() != 1
            || v["line_items"]["has_more"] != false
        {
            return Err(reconcile());
        }
        let owner = checkout::required(&v, "client_reference_id")?;
        let amount = integer(&v, "amount_total")?;
        let charge_id = id(&charge["id"])?;
        if v["metadata"]["account_id"] != owner
            || amount != integer(charge, "amount")?
            || charge["currency"] != "usd"
            || event.charge_id.as_deref().is_some_and(|id| id != charge_id)
        {
            return Err(reconcile());
        }
        if self.credit_price.as_deref() != Some(id(&items[0]["price"])?) {
            return Err(reconcile());
        }
        let mut refunds = vec![];
        let mut cursor: Option<String> = None;
        loop {
            let mut request = self
                .http
                .request(reqwest::Method::GET, &["refunds"])?
                .query(&[("charge", charge_id), ("limit", "100")]);
            if let Some(cursor) = &cursor {
                request = request.query(&[("starting_after", cursor)]);
            }
            let page = ProviderHttp::json(request).await?;
            let data = page["data"].as_array().ok_or_else(reconcile)?;
            for refund in data {
                match refund["status"].as_str() {
                    Some("succeeded") => {
                        let refund_id = id(&refund["id"])?.to_owned();
                        let amount = integer(refund, "amount")?;
                        if let Some((_, old)) = refunds.iter().find(|(old, _)| old == &refund_id) {
                            if *old != amount {
                                return Err(reconcile());
                            }
                        } else {
                            refunds.push((refund_id, amount));
                        }
                    }
                    Some("failed" | "canceled" | "requires_action") => return Err(reconcile()),
                    Some("pending") => {}
                    _ => return Err(reconcile()),
                }
            }
            let more = page["has_more"].as_bool().ok_or_else(reconcile)?;
            if !more {
                break;
            }
            let next = id(&data.last().ok_or_else(reconcile)?["id"])?;
            if cursor.as_deref() == Some(next) || refunds.len() > 10000 {
                return Err(reconcile());
            }
            cursor = Some(next.into());
        }
        Ok(Some(CreditPurchase {
            id: session,
            owner: owner.into(),
            customer: id(&v["customer"])?.into(),
            amount,
            paid: v["payment_status"] == "paid"
                && charge["paid"] == true
                && charge["captured"] == true
                && integer(charge, "amount_captured")? == amount
                && intent["status"] == "succeeded"
                && integer(intent, "amount_received")? == amount,
            currency: checkout::required(&v, "currency")?.into(),
            price: id(&items[0]["price"])?.into(),
            quantity: integer(&items[0], "quantity")?,
            charge: charge_id.into(),
            disputed: charge["disputed"].as_bool().ok_or_else(reconcile)?,
            refunds,
        }))
    }
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Adjustment<'a> {
    customer_id: &'a str,
    amount: i64,
    purchase_id: &'a str,
}
impl BillingControl {
    async fn adjust_credit(
        &self,
        provider: &dyn PortalProvider,
        purchase: &CreditPurchase,
        key: &str,
        amount: i64,
        now: i64,
    ) -> Result<(), ApiError> {
        // Preserve the original JSON field order and exact keys for existing delivery receipts.
        let payload = serde_json::to_string(&Adjustment {
            customer_id: &purchase.customer,
            amount,
            purchase_id: &purchase.id,
        })
        .map_err(|_| reconcile())?;
        let mut tx = self.effects_pool.begin().await?;
        let row=sqlx::query("INSERT INTO api_stripe_meter_deliveries(delivery_key,payload,first_attempt_at,lease_until) VALUES($1,$2,$3,0) ON CONFLICT(delivery_key) DO UPDATE SET delivery_key=EXCLUDED.delivery_key RETURNING payload,sent_at,first_attempt_at,lease_until").bind(key).bind(&payload).bind(now).fetch_one(&mut *tx).await?;
        let stored: String = row.try_get("payload")?;
        if stored != payload {
            return Err(reconcile());
        }
        if row.try_get::<Option<i64>, _>("sent_at")?.is_some() {
            tx.commit().await?;
            return Ok(());
        }
        let first: i64 = row.try_get("first_attempt_at")?;
        if now.saturating_sub(first) >= 23 * 3600000 {
            return Err(reconcile());
        }
        if row.try_get::<i64, _>("lease_until")? > now {
            return Err(ApiError::unavailable("billing_credit_delivery_busy"));
        }
        sqlx::query("UPDATE api_stripe_meter_deliveries SET lease_until=$2 WHERE delivery_key=$1")
            .bind(key)
            .bind(now + 60000)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        let result = provider
            .credit_adjustment(&purchase.customer, amount, key, &purchase.id)
            .await;
        sqlx::query("UPDATE api_stripe_meter_deliveries SET lease_until=0,sent_at=CASE WHEN $2 THEN $3 ELSE sent_at END WHERE delivery_key=$1").bind(key).bind(result.is_ok()).bind(now).execute(&self.effects_pool).await?;
        result
    }
    pub(super) async fn process_credit(
        &self,
        provider: &dyn PortalProvider,
        event: &CreditEvent,
        now: i64,
    ) -> Result<Value, ApiError> {
        if provider.provider_id() != "stripe" {
            return Err(unavailable());
        }
        let Some(purchase) = provider.credit_purchase(event).await? else {
            return Ok(json!({"ok":true,"prepaid":true}));
        };
        let mut tx = self.owned_transaction(&purchase.owner).await?;
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,68130))")
            .bind(&purchase.owner)
            .execute(&mut *tx)
            .await?;
        let customer: Option<String> =
            sqlx::query_scalar("SELECT customer_id FROM api_stripe_customers WHERE account_id=$1")
                .bind(&purchase.owner)
                .fetch_optional(&mut *tx)
                .await?
                .flatten();
        if customer.as_deref() != Some(purchase.customer.as_str()) {
            return Err(reconcile());
        }
        provider
            .verify_customer(&purchase.owner, &purchase.customer)
            .await?;
        // The adapter validates its configured price as well as canonical captured payment evidence.
        if !PREPAID_CREDIT_AMOUNTS.contains(&purchase.amount)
            || purchase.currency != "usd"
            || purchase.quantity.checked_mul(100) != Some(purchase.amount)
        {
            return Err(reconcile());
        }
        if !purchase.paid {
            tx.commit().await?;
            return Ok(json!({"ok":true,"prepaid":true}));
        }
        let sum = purchase
            .refunds
            .iter()
            .try_fold(0_i64, |sum, (_, amount)| {
                if *amount > 0 {
                    sum.checked_add(*amount)
                } else {
                    None
                }
            })
            .ok_or_else(reconcile)?;
        if sum > purchase.amount || (purchase.disputed && !purchase.refunds.is_empty()) {
            return Err(reconcile());
        }
        if let Some(dispute) = &event.dispute {
            if !purchase.refunds.is_empty()
                || dispute.amount <= 0
                || dispute.amount > purchase.amount
            {
                return Err(reconcile());
            }
        }
        self.adjust_credit(
            provider,
            &purchase,
            &format!("prepaid:purchase:{}", purchase.id),
            -purchase.amount,
            now,
        )
        .await?;
        for (refund, amount) in &purchase.refunds {
            self.adjust_credit(
                provider,
                &purchase,
                &format!("prepaid:refund:{refund}"),
                *amount,
                now,
            )
            .await?;
        }
        if let Some(dispute) = &event.dispute {
            self.adjust_credit(
                provider,
                &purchase,
                &format!("prepaid:dispute:{}:withdrawn", dispute.id),
                dispute.amount,
                now,
            )
            .await?;
            if dispute.reinstated {
                self.adjust_credit(
                    provider,
                    &purchase,
                    &format!("prepaid:dispute:{}:reinstated", dispute.id),
                    -dispute.amount,
                    now,
                )
                .await?;
            }
        }
        tx.commit().await?;
        Ok(json!({"ok":true,"prepaid":true}))
    }
    async fn credit_provider(&self, owner: &str) -> Result<Arc<dyn PortalProvider>, ApiError> {
        let providers = self.payments.as_ref().ok_or_else(unavailable)?;
        let ids:Vec<String>=sqlx::query_scalar("SELECT DISTINCT provider FROM api_entitlements WHERE account_id=$1 AND provider_subscription_id IS NOT NULL AND status<>'ended'").bind(owner).fetch_all(&self.effects_pool).await?;
        if ids.len() > 1 {
            return Err(ApiError::conflict("billing_provider_ambiguous"));
        }
        let id = ids.first().unwrap_or(&providers.default);
        providers.providers.get(id).cloned().ok_or_else(unavailable)
    }
    async fn prepaid_balance(&self, owner: &str) -> Result<PrepaidBalance, ApiError> {
        let provider = self.credit_provider(owner).await?;
        let customer: Option<String> = if provider.provider_id() == "stripe" {
            sqlx::query_scalar("SELECT customer_id FROM api_stripe_customers WHERE account_id=$1")
                .bind(owner)
                .fetch_optional(&self.pool)
                .await?
                .flatten()
        } else {
            None
        };
        let mut value = provider.prepaid_balance(owner, customer.as_deref()).await?;
        value.available &= self.payments.as_ref().is_some_and(|p| p.checkout.is_some());
        Ok(value)
    }
    async fn prepaid_checkout(
        &self,
        owner: &str,
        amount: i64,
        now: i64,
    ) -> Result<String, ApiError> {
        if !PREPAID_CREDIT_AMOUNTS.contains(&amount) {
            return Err(ApiError::bad_request("invalid_prepaid_amount"));
        }
        let providers = self.payments.as_ref().ok_or_else(unavailable)?;
        let settings = providers
            .checkout
            .as_ref()
            .ok_or_else(|| ApiError::unavailable("billing_approval_pending"))?;
        let mut tx = self.owned_transaction(owner).await?;
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,68130))")
            .bind(owner)
            .execute(&mut *tx)
            .await?;
        let provider = self.credit_provider(owner).await?;
        if !provider.prepaid_available() {
            return Err(ApiError::unavailable("billing_approval_pending"));
        }
        let customer =
            checkout::stripe_customer(&self.effects_pool, provider.as_ref(), owner, now).await?;
        let success = format!("{}/v1/billing/prepaid/complete", settings.signer.issuer());
        let url = provider
            .prepaid_checkout(owner, &customer, amount, &success)
            .await?;
        secure_url(&url)?;
        tx.commit().await?;
        Ok(url)
    }
}
impl Store {
    pub async fn prepaid_balance(&self, owner: &str) -> Result<PrepaidBalance, ApiError> {
        self.billing
            .as_ref()
            .ok_or_else(|| ApiError::unavailable("cloud_billing_writes_disabled"))?
            .prepaid_balance(owner)
            .await
    }
    pub async fn prepaid_checkout(
        &self,
        owner: &str,
        amount: i64,
        now: i64,
    ) -> Result<String, ApiError> {
        self.billing
            .as_ref()
            .ok_or_else(|| ApiError::unavailable("cloud_billing_writes_disabled"))?
            .prepaid_checkout(owner, amount, now)
            .await
    }
}

#[cfg(test)]
mod tests;
