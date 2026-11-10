//! Signed delivery is a notification. Canonical provider state authorizes subscription changes.
use super::*;
use axum::http::HeaderMap;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};
use sqlx::Row;

#[derive(Clone, Debug)]
pub enum PaymentEvent {
    Subscription {
        event_id: String,
        subscription_id: String,
    },
    Prepaid(CreditEvent),
}
impl PaymentEvent {
    pub fn event_id(&self) -> &str {
        match self {
            Self::Subscription { event_id, .. } => event_id,
            Self::Prepaid(event) => &event.event_id,
        }
    }
    pub fn subscription_id(&self) -> Option<&str> {
        match self {
            Self::Subscription {
                subscription_id, ..
            } => Some(subscription_id),
            _ => None,
        }
    }
}
#[derive(Clone, Debug)]
pub struct Subscription {
    pub owner: String,
    pub id: String,
    pub offer: String,
    pub status: String,
    pub period_start: i64,
    pub paid_through: i64,
    pub customer: Option<String>,
}
pub(super) struct EventConfig {
    secret: Zeroizing<String>,
    offers: HashMap<String, String>,
}
fn invalid() -> ApiError {
    ApiError::unauthorized("invalid_billing_event")
}
fn text<'a>(v: &'a Value, key: &str) -> Result<&'a str, ApiError> {
    v[key]
        .as_str()
        .filter(|s| !s.is_empty() && s.len() <= 1024)
        .ok_or_else(unavailable)
}
fn header<'a>(h: &'a HeaderMap, key: &str) -> Result<&'a str, ApiError> {
    h.get(key)
        .and_then(|v| v.to_str().ok())
        .filter(|v| v.len() <= 8192)
        .ok_or_else(invalid)
}
fn recent(timestamp: &str, now: i64) -> Result<(), ApiError> {
    let t = timestamp.parse::<i64>().map_err(|_| invalid())?;
    if t.abs_diff(now / 1000) > 300 {
        return Err(invalid());
    }
    Ok(())
}
fn signature(key: &[u8], prefix: &str, body: &[u8], expected: &[u8]) -> bool {
    let Ok(mut mac) = Hmac::<Sha256>::new_from_slice(key) else {
        return false;
    };
    mac.update(prefix.as_bytes());
    mac.update(body);
    mac.verify_slice(expected).is_ok()
}
fn hex(value: &str) -> Option<Vec<u8>> {
    if value.len() != 64 {
        return None;
    }
    (0..64)
        .step_by(2)
        .map(|i| u8::from_str_radix(value.get(i..i + 2)?, 16).ok())
        .collect()
}
fn status(value: &str) -> String {
    match value {
        "active" | "trialing" => "active",
        "past_due" => "grace",
        "incomplete" => "pending",
        _ => "ended",
    }
    .into()
}
impl EventConfig {
    #[cfg(test)]
    pub(super) fn fixture(secret: &str, price: &str) -> Self {
        Self {
            secret: Zeroizing::new(secret.into()),
            offers: HashMap::from([(price.into(), "cloud-workspace-standard-v1".into())]),
        }
    }

    pub(super) fn from_env(prefix: &str, kind: &str) -> anyhow::Result<Option<Self>> {
        let secret = std::env::var(format!("{prefix}_WEBHOOK_SECRET")).unwrap_or_default();
        if secret.trim().is_empty() {
            return Ok(None);
        }
        let mut offers = HashMap::new();
        for (suffix, offer) in [
            ("CLOUD_WORKSPACE_STANDARD_V1", "cloud-workspace-standard-v1"),
            ("PERSISTENT_STANDARD_V1", "persistent-standard-v1"),
        ] {
            let configured = std::env::var(format!("{prefix}_{kind}_{suffix}"))
                .ok()
                .or_else(|| {
                    if prefix == "POLAR" && suffix == "CLOUD_WORKSPACE_STANDARD_V1" {
                        std::env::var("POLAR_PRODUCT_SANDBOX_STANDARD_V1").ok()
                    } else {
                        None
                    }
                });
            if let Some(id) = configured {
                if !id.trim().is_empty() {
                    anyhow::ensure!(
                        offers.insert(id, offer.into()).is_none(),
                        "Duplicate billing offer identifier"
                    );
                }
            }
        }
        anyhow::ensure!(
            !offers.is_empty(),
            "Billing webhook requires configured offers"
        );
        Ok(Some(Self {
            secret: Zeroizing::new(secret),
            offers,
        }))
    }
    pub(super) fn stripe_event(
        &self,
        h: &HeaderMap,
        body: &[u8],
        now: i64,
    ) -> Result<Option<PaymentEvent>, ApiError> {
        let fields: Vec<_> = header(h, "stripe-signature")?
            .split(',')
            .filter_map(|s| s.trim().split_once('='))
            .collect();
        let ts = fields
            .iter()
            .find(|(k, _)| *k == "t")
            .map(|(_, v)| *v)
            .ok_or_else(invalid)?;
        recent(ts, now)?;
        if !fields
            .iter()
            .filter(|(k, _)| *k == "v1")
            .filter_map(|(_, v)| hex(v))
            .any(|expected| signature(self.secret.as_bytes(), &format!("{ts}."), body, &expected))
        {
            return Err(invalid());
        }
        let v: Value = serde_json::from_slice(body).map_err(|_| invalid())?;
        let event_id = text(&v, "id").map_err(|_| invalid())?.to_owned();
        let object = &v["data"]["object"];
        if let Some(event) = CreditEvent::from_verified(&event_id, &v)? {
            return Ok(Some(PaymentEvent::Prepaid(event)));
        }

        let id = match v["type"].as_str() {
            Some(
                "customer.subscription.created"
                | "customer.subscription.updated"
                | "customer.subscription.deleted"
                | "customer.subscription.paused"
                | "customer.subscription.resumed",
            ) => Some(text(object, "id").map_err(|_| invalid())?),
            Some("invoice.paid" | "invoice.payment_failed") => {
                let id = &object["parent"]["subscription_details"]["subscription"];
                id.as_str().or_else(|| id["id"].as_str())
            }
            _ => None,
        };
        Ok(id.map(|id| PaymentEvent::Subscription {
            event_id,
            subscription_id: id.into(),
        }))
    }
    pub(super) fn polar_event(
        &self,
        h: &HeaderMap,
        body: &[u8],
        now: i64,
    ) -> Result<Option<PaymentEvent>, ApiError> {
        let event_id = header(h, "webhook-id")?;
        let ts = header(h, "webhook-timestamp")?;
        recent(ts, now)?;
        if event_id.is_empty() || event_id.len() > 1024 {
            return Err(invalid());
        }
        // Original Polar SDK signs with literal UTF-8 secret bytes, including its prefix.
        let valid = header(h, "webhook-signature")?
            .split_whitespace()
            .filter_map(|v| v.strip_prefix("v1,"))
            .filter_map(|v| STANDARD.decode(v).ok())
            .any(|expected| {
                signature(
                    self.secret.as_bytes(),
                    &format!("{event_id}.{ts}."),
                    body,
                    &expected,
                )
            });
        // New Standard Webhooks secrets encode their key after whsec_. Preserve
        // literal-key verification for existing Polar endpoints at the pinned reference.
        let standard_key = self
            .secret
            .strip_prefix("whsec_")
            .and_then(|s| STANDARD.decode(s).ok())
            .map(Zeroizing::new);
        let valid = valid
            || standard_key.as_ref().is_some_and(|key| {
                header(h, "webhook-signature").is_ok_and(|value| {
                    value
                        .split_whitespace()
                        .filter_map(|v| v.strip_prefix("v1,"))
                        .filter_map(|v| STANDARD.decode(v).ok())
                        .any(|expected| {
                            signature(key, &format!("{event_id}.{ts}."), body, &expected)
                        })
                })
            });
        if !valid {
            return Err(invalid());
        }
        let v: Value = serde_json::from_slice(body).map_err(|_| invalid())?;
        match v["type"].as_str() {
            Some(
                "subscription.active"
                | "subscription.canceled"
                | "subscription.created"
                | "subscription.past_due"
                | "subscription.revoked"
                | "subscription.uncanceled"
                | "subscription.updated",
            ) => Ok(Some(PaymentEvent::Subscription {
                event_id: event_id.into(),
                subscription_id: text(&v["data"], "id").map_err(|_| invalid())?.into(),
            })),
            _ => Ok(None),
        }
    }
    pub(super) fn stripe_subscription(
        &self,
        id: &str,
        v: &Value,
    ) -> Result<Subscription, ApiError> {
        if v["id"] != id || v["customer"]["deleted"] == true {
            return Err(unavailable());
        }
        let customer = &v["customer"];
        let owner = customer["metadata"]["account_id"]
            .as_str()
            .filter(|v| !v.trim().is_empty())
            .ok_or_else(|| ApiError::conflict("subscription_unlinked"))?;
        let items = v["items"]["data"].as_array().ok_or_else(unavailable)?;
        let base: Vec<_> = items
            .iter()
            .filter(|i| {
                i["price"]["id"]
                    .as_str()
                    .is_some_and(|p| self.offers.contains_key(p))
            })
            .collect();
        if base.len() != 1 {
            return Err(unavailable());
        }
        let item = base[0];
        let offer = &self.offers[item["price"]["id"].as_str().unwrap()];
        if v["metadata"]["account_id"]
            .as_str()
            .is_some_and(|s| s != owner)
            || v["metadata"]["offer_id"]
                .as_str()
                .is_some_and(|s| s != offer)
        {
            return Err(unavailable());
        }
        let start = item["current_period_start"]
            .as_i64()
            .and_then(|v| v.checked_mul(1000))
            .ok_or_else(unavailable)?;
        let end = item["current_period_end"]
            .as_i64()
            .and_then(|v| v.checked_mul(1000))
            .ok_or_else(unavailable)?;
        let sub = Subscription {
            owner: owner.into(),
            id: id.into(),
            offer: offer.clone(),
            status: status(text(v, "status")?),
            period_start: start,
            paid_through: end,
            customer: Some(text(customer, "id")?.into()),
        };
        sub.validate()?;
        Ok(sub)
    }
    pub(super) fn polar_subscription(&self, id: &str, v: &Value) -> Result<Subscription, ApiError> {
        if v["id"] != id {
            return Err(unavailable());
        }
        let owner = v["customer"]["external_id"]
            .as_str()
            .or_else(|| v["metadata"]["account_id"].as_str())
            .filter(|v| !v.trim().is_empty())
            .ok_or_else(|| ApiError::conflict("subscription_unlinked"))?;
        let offer = self
            .offers
            .get(text(v, "product_id")?)
            .ok_or_else(unavailable)?;
        if v["metadata"]["offer_id"]
            .as_str()
            .is_some_and(|v| v != offer)
        {
            return Err(unavailable());
        }
        let date = |key| {
            chrono::DateTime::parse_from_rfc3339(text(v, key)?)
                .map(|d| d.timestamp_millis())
                .map_err(|_| unavailable())
        };
        let sub = Subscription {
            owner: owner.into(),
            id: id.into(),
            offer: offer.clone(),
            status: status(text(v, "status")?),
            period_start: date("current_period_start")?,
            paid_through: date("current_period_end")?,
            customer: None,
        };
        sub.validate()?;
        Ok(sub)
    }
}
impl Subscription {
    fn validate(&self) -> Result<(), ApiError> {
        if self.owner.trim().is_empty()
            || self.owner.len() > 1024
            || self.id.is_empty()
            || self.id.len() > 1024
            || self.period_start < 0
            || self.paid_through <= self.period_start
            || self.paid_through > 9_007_199_254_740_991
            || !matches!(
                self.status.as_str(),
                "pending" | "active" | "grace" | "ended"
            )
        {
            return Err(unavailable());
        }
        Ok(())
    }
}
impl BillingControl {
    async fn webhook(
        &self,
        provider_id: Option<&str>,
        headers: &HeaderMap,
        body: &[u8],
        now: i64,
    ) -> Result<Value, ApiError> {
        if let Some(result) = self.native_webhook(provider_id, headers, body).await? {
            return Ok(result);
        }
        let providers = self.payments.as_ref().ok_or_else(unavailable)?;
        let id = match provider_id {
            Some(id) => id,
            None if providers.providers.len() == 1 => providers.providers.keys().next().unwrap(),
            None => return Err(ApiError::bad_request("billing_provider_required")),
        };
        let provider = providers.providers.get(id).ok_or_else(unavailable)?;
        let Some(event) = provider.verify_event(headers, body, now)? else {
            return Ok(json!({"ok":true,"ignored":true}));
        };
        let (event_id, subscription_id) = match event {
            PaymentEvent::Prepaid(event) => {
                return self.process_credit(provider.as_ref(), &event, now).await;
            }
            PaymentEvent::Subscription {
                event_id,
                subscription_id,
            } => (event_id, subscription_id),
        };
        if event_id.is_empty()
            || event_id.len() > 1024
            || subscription_id.is_empty()
            || subscription_id.len() > 1024
        {
            return Err(invalid());
        }
        let mut tx = self.pool.begin().await?;
        // Serialize before fetching canonical state, so old/parallel notifications cannot rewind it.
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,68129))")
            .bind(format!("{id}:{}", subscription_id))
            .execute(&mut *tx)
            .await?;
        let sub = match provider.subscription(&subscription_id).await {
            Err(e) if e.code == "subscription_unlinked" => {
                return Ok(json!({"ok":true,"deferred":"subscription-unlinked"}));
            }
            other => other?,
        };
        sub.validate()?;
        if sub.id != subscription_id {
            return Err(unavailable());
        }
        if !self.accounts.contains(&sub.owner) {
            return Err(ApiError::forbidden("cloud_billing_writes_disabled"));
        }
        let backend:Option<String>=sqlx::query_scalar("SELECT backend FROM zuse_rust_control_owners WHERE resource_kind='billing' AND resource_id=$1 FOR SHARE").bind(&sub.owner).fetch_optional(&mut *tx).await?;
        if backend.as_deref() != Some("rust") {
            return Err(ApiError::conflict("cloud_billing_owner_not_rust"));
        }
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,68130))")
            .bind(&sub.owner)
            .execute(&mut *tx)
            .await?;
        if id == "stripe" {
            let customer: Option<String> = sqlx::query_scalar(
                "SELECT customer_id FROM api_stripe_customers WHERE account_id=$1",
            )
            .bind(&sub.owner)
            .fetch_optional(&mut *tx)
            .await?
            .flatten();
            if customer.is_none() {
                return Ok(json!({"ok":true,"deferred":"subscription-unlinked"}));
            }
            if customer != sub.customer {
                return Err(unavailable());
            }
        }
        // Persistent subscription processing also needs the machine lifecycle writer (separate milestone).
        if sub.offer != "cloud-workspace-standard-v1" {
            return Err(ApiError::unavailable(
                "billing_machine_processing_unavailable",
            ));
        }
        let result = self
            .project_subscription(&mut tx, id, &event_id, &sub, now, true)
            .await?;
        tx.commit().await?;
        Ok(result)
    }
    pub(crate) async fn project_subscription(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        id: &str,
        event_id: &str,
        sub: &Subscription,
        now: i64,
        deduplicate: bool,
    ) -> Result<Value, ApiError> {
        let consumed:Option<String>=sqlx::query_scalar("INSERT INTO api_billing_events(provider,event_id,payload,received_at,processed_at) VALUES($1,$2,NULL,$3,$3) ON CONFLICT(provider,event_id) DO NOTHING RETURNING event_id").bind(id).bind(&event_id).bind(now).fetch_optional(&mut **tx).await?;
        if consumed.is_none() && deduplicate {
            return Ok(json!({"ok":true,"duplicate":true,"provisioningQueued":false}));
        }
        let existing=sqlx::query("SELECT entitlement_id,account_id FROM api_entitlements WHERE provider=$1 AND provider_subscription_id=$2 FOR UPDATE").bind(id).bind(&sub.id).fetch_all(&mut **tx).await?;
        if existing.len() > 1
            || existing
                .first()
                .is_some_and(|r| r.get::<String, _>("account_id") != sub.owner)
        {
            return Err(ApiError::conflict("billing_subscription_owner_conflict"));
        }
        let entitlement = existing
            .first()
            .map(|r| r.get::<String, _>("entitlement_id"))
            .unwrap_or_else(|| {
                format!(
                    "ent_{}",
                    &format!("{:x}", Sha256::digest(format!("{id}:{}", sub.id)))[..24]
                )
            });
        let ended = if sub.status == "ended" && sub.paid_through <= now {
            Some(now)
        } else {
            None
        };
        let written:Option<String>=sqlx::query_scalar("INSERT INTO api_entitlements(entitlement_id,account_id,kind,offer_id,provider,provider_subscription_id,status,period_start,paid_through,ended_at,credit_balance,created_at,updated_at) VALUES($1,$2,'cloud-workspace',$3,$4,$5,$6,$7,$8,$9,NULL,$10,$10) ON CONFLICT(entitlement_id) DO UPDATE SET kind=EXCLUDED.kind,offer_id=EXCLUDED.offer_id,status=EXCLUDED.status,period_start=EXCLUDED.period_start,paid_through=EXCLUDED.paid_through,ended_at=EXCLUDED.ended_at,updated_at=EXCLUDED.updated_at WHERE api_entitlements.account_id=EXCLUDED.account_id AND api_entitlements.provider=EXCLUDED.provider AND api_entitlements.provider_subscription_id=EXCLUDED.provider_subscription_id RETURNING entitlement_id").bind(&entitlement).bind(&sub.owner).bind(&sub.offer).bind(id).bind(&sub.id).bind(&sub.status).bind(sub.period_start).bind(sub.paid_through).bind(ended).bind(now).fetch_optional(&mut **tx).await?;
        if written.is_none() {
            return Err(ApiError::conflict("billing_subscription_owner_conflict"));
        }
        let period_status = match sub.status.as_str() {
            "pending" | "grace" => "billing-hold",
            "ended" if sub.paid_through <= now => "ended",
            _ => "active",
        };
        let period = format!("cloud:{}:{}", sub.owner, sub.period_start);
        let written:Option<String>=sqlx::query_scalar("INSERT INTO api_cloud_billing_periods(period_id,account_id,provider_subscription_id,billing_provider,status,currency,period_start,period_end,base_price_micros,included_provider_cost_micros,markup_basis_points,price_catalog_version,overage_cap_micros,created_at,updated_at) VALUES($1,$2,$3,$4,$5,'USD',$6,$7,40000000,35000000,500,'provider-compute-v1',COALESCE((SELECT overage_cap_micros FROM api_cloud_billing_periods WHERE account_id=$2 AND period_start<$6 ORDER BY period_start DESC LIMIT 1),25000000),$8,$8) ON CONFLICT(account_id,period_start) DO UPDATE SET status=EXCLUDED.status,period_end=EXCLUDED.period_end,provider_subscription_id=EXCLUDED.provider_subscription_id,updated_at=EXCLUDED.updated_at WHERE api_cloud_billing_periods.billing_provider=EXCLUDED.billing_provider AND (api_cloud_billing_periods.provider_subscription_id IS NULL OR api_cloud_billing_periods.provider_subscription_id=EXCLUDED.provider_subscription_id) RETURNING period_id").bind(period).bind(&sub.owner).bind(&sub.id).bind(id).bind(period_status).bind(sub.period_start).bind(sub.paid_through).bind(now).fetch_optional(&mut **tx).await?;
        if written.is_none() {
            return Err(ApiError::conflict("billing_period_owner_conflict"));
        }
        Ok(json!({"ok":true,"duplicate":false,"provisioningQueued":false}))
    }
}
impl Store {
    pub async fn billing_webhook(
        &self,
        provider: Option<&str>,
        headers: &HeaderMap,
        body: &[u8],
        now: i64,
    ) -> Result<Value, ApiError> {
        self.billing
            .as_ref()
            .ok_or_else(|| ApiError::unavailable("cloud_billing_writes_disabled"))?
            .webhook(provider, headers, body, now)
            .await
    }
}

#[cfg(test)]
mod tests;
