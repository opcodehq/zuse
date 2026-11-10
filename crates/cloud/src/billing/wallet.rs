//! Prepaid credit on the embedded brokeboy ledger. One credit is one US dollar, and
//! amounts cross this boundary as integer micros.
use crate::error::ApiError;
use axum::http::{HeaderMap, StatusCode};
use brokeboy::{
    Billing, BillingOptions,
    core::{BillingError, decimal::Decimal, models::DatabaseConnection, schema::Dialect},
    operations::Environment,
    stripe::{Stripe, StripeOptions},
};
use serde_json::{Value, json};
use std::{collections::BTreeMap, sync::Arc};

pub const MIGRATION: &str = include_str!("../../migrations/0033_brokeboy.sql");
const FEATURE: &str = "wallet";

fn insufficient() -> ApiError {
    ApiError::new(
        StatusCode::PAYMENT_REQUIRED,
        "hosted_models_insufficient_credits",
    )
}
fn failed(error: BillingError) -> ApiError {
    tracing::warn!(code = %error.code, "Wallet operation failed");
    ApiError::unavailable("billing_wallet_unavailable")
}
fn amount(micros: i64) -> Result<String, ApiError> {
    Decimal::from_scaled(i128::from(micros))
        .map(|value| value.to_string())
        .map_err(failed)
}
fn micros(value: &str) -> Result<i64, ApiError> {
    let value = value.parse::<Decimal>().map_err(failed)?;
    i64::try_from(value.scaled()).map_err(|_| ApiError::unavailable("billing_wallet_unavailable"))
}
/// Only events that can move wallet credit. Subscription traffic stays with the
/// existing lifecycle code, and legacy credit purchases carry no wallet metadata.
fn handles(event: &Value) -> bool {
    match event["type"].as_str() {
        Some("checkout.session.completed" | "checkout.session.async_payment_succeeded") => {
            event["data"]["object"]["metadata"]["brokeboy_prepaid_feature"].is_string()
        }
        Some(
            "charge.refunded"
            | "refund.updated"
            | "refund.failed"
            | "charge.dispute.created"
            | "charge.dispute.funds_withdrawn"
            | "charge.dispute.funds_reinstated",
        ) => true,
        _ => false,
    }
}

#[derive(Clone)]
pub struct Wallet(Billing);

impl Wallet {
    pub fn enabled() -> bool {
        std::env::var("BROKEBOY_ENABLED").as_deref() == Ok("true")
    }
    pub async fn from_env(database_url: &str) -> anyhow::Result<Option<Self>> {
        if !Self::enabled() {
            return Ok(None);
        }
        let required = |key: &str| {
            std::env::var(key)
                .ok()
                .filter(|v| !v.trim().is_empty())
                .ok_or_else(|| anyhow::anyhow!("The wallet requires {key}"))
        };
        let secret = required("STRIPE_SECRET_KEY")?;
        let mut stripe = StripeOptions::new(secret, required("STRIPE_WEBHOOK_SECRET")?);
        if stripe.secret_key.starts_with("sk_live_") {
            stripe.environment = Environment::Live;
        }
        Self::open(database_url, stripe).await.map(Some)
    }
    async fn open(database_url: &str, stripe: StripeOptions) -> anyhow::Result<Self> {
        let environment = stripe.environment;
        let provider = Stripe::new(stripe)
            .map_err(|e| anyhow::anyhow!("Invalid wallet Stripe configuration: {}", e.code))?;
        let billing = Billing::open(
            DatabaseConnection {
                driver: Dialect::Postgres,
                url: database_url.into(),
                max_connections: Some(4),
                acquire_timeout_ms: Some(5000),
            },
            BillingOptions {
                owns_provider: true,
                provider: Some(Arc::new(provider)),
                environment,
                ..Default::default()
            },
        )
        .await
        .map_err(|e| anyhow::anyhow!("Cannot open the wallet: {}", e.code))?;
        Ok(Self(billing))
    }
    pub(super) async fn spendable_micros(&self, owner: &str) -> Result<i64, ApiError> {
        match self
            .0
            .credits()
            .balance(json!({"customerId":owner,"feature":FEATURE}))
            .await
        {
            Ok(balance) => micros(&balance.available).map(|v| v.max(0)),
            Err(error) if error.code == "customer_not_found" => Ok(0),
            Err(error) => Err(failed(error)),
        }
    }
    pub(super) async fn hold(
        &self,
        owner: &str,
        request: &str,
        hold_micros: i64,
        expires_at: i64,
    ) -> Result<String, ApiError> {
        match self
            .0
            .reservations()
            .create(json!({"customerId":owner,"feature":FEATURE,"amount":amount(hold_micros)?,"expiresAt":expires_at,"idempotencyKey":format!("hosted:{request}:hold")}))
            .await
        {
            Ok(reservation) => Ok(reservation.id),
            Err(error) if matches!(error.code.as_str(), "insufficient_balance" | "customer_not_found") => {
                Err(insufficient())
            }
            Err(error) => Err(failed(error)),
        }
    }
    /// False when the wallet cannot cover the extra amount.
    pub(super) async fn increase(
        &self,
        owner: &str,
        reservation: &str,
        request: &str,
        extra_micros: i64,
    ) -> Result<bool, ApiError> {
        match self
            .0
            .reservations()
            .increase(json!({"customerId":owner,"reservationId":reservation,"amount":amount(extra_micros)?,"idempotencyKey":format!("hosted:{request}:increase")}))
            .await
        {
            Ok(_) => Ok(true),
            Err(error) if error.code == "insufficient_balance" => Ok(false),
            Err(error) => Err(failed(error)),
        }
    }
    /// False when the ledger routed the settlement to reconciliation instead of spending.
    pub(super) async fn settle(
        &self,
        owner: &str,
        reservation: &str,
        request: &str,
        cost_micros: i64,
    ) -> Result<bool, ApiError> {
        let result = self
            .0
            .reservations()
            .settle(json!({"customerId":owner,"reservationId":reservation,"amount":amount(cost_micros)?,"idempotencyKey":format!("hosted:{request}:settle:{cost_micros}")}))
            .await
            .map_err(failed)?;
        Ok(result["status"] == "settled")
    }
    /// Links the existing Stripe customer, keeping its ID, then opens a credit checkout.
    pub(super) async fn checkout(
        &self,
        owner: &str,
        stripe_customer: &str,
        amount_cents: i64,
        success: &str,
    ) -> Result<String, ApiError> {
        self.0
            .migration()
            .adopt_stripe_customer(json!({"customerId":owner,"stripeCustomerId":stripe_customer,"dryRun":false,"idempotencyKey":format!("zuse-link-{stripe_customer}")}))
            .await
            .map_err(failed)?;
        let work = self
            .0
            .call(
                "prepaid.checkout",
                json!({"customerId":owner,"feature":FEATURE,"amountCents":amount_cents,"successUrl":success,"idempotencyKey":uuid::Uuid::new_v4().to_string()}),
            )
            .await
            .map_err(failed)?;
        work["result"]["url"]
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| ApiError::unavailable("billing_wallet_unavailable"))
    }
    /// Errors make Stripe redeliver; the ledger inbox ignores events it already holds.
    pub(super) async fn webhook(&self, headers: &HeaderMap, body: &[u8]) -> Result<(), ApiError> {
        let Ok(event) = serde_json::from_slice::<Value>(body) else {
            return Ok(());
        };
        if !handles(&event) {
            return Ok(());
        }
        let headers: BTreeMap<String, String> = headers
            .iter()
            .filter_map(|(k, v)| Some((k.as_str().to_owned(), v.to_str().ok()?.to_owned())))
            .collect();
        self.0
            .webhook(headers, body.to_vec())
            .await
            .map_err(failed)?;
        // Credit the buyer now when possible. The billing worker covers everything else.
        if let Some(customer) = event["data"]["object"]["metadata"]["brokeboy_customer"].as_str() {
            if let Err(error) = self
                .0
                .maintenance()
                .run(json!({"customerId":customer,"limit":25}))
                .await
            {
                tracing::warn!(code = %error.code, "Wallet maintenance deferred");
            }
        }
        Ok(())
    }
    pub(super) async fn maintain(&self, limit: u32) -> Result<(), ApiError> {
        self.0
            .maintenance()
            .run(json!({"limit":limit}))
            .await
            .map(|_| ())
            .map_err(failed)
    }
    #[cfg(test)]
    pub(super) async fn fixture(database_url: &str) -> Self {
        Self::open(
            database_url,
            StripeOptions::new("sk_test_fixture".into(), "whsec_fixture".into()),
        )
        .await
        .unwrap()
    }
    #[cfg(test)]
    pub(super) async fn fund(&self, owner: &str, dollars: &str) {
        self.0
            .customers()
            .create(json!({"id":owner,"idempotencyKey":format!("fixture-{owner}")}))
            .await
            .unwrap();
        self.0
            .credits()
            .grant(json!({"customerId":owner,"feature":FEATURE,"amount":dollars,"idempotencyKey":format!("fixture-grant-{owner}")}))
            .await
            .unwrap();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use brokeboy::core::schema::migration_sql;

    #[tokio::test]
    #[ignore = "requires explicit isolated localhost PostgreSQL"]
    async fn signed_checkout_credits_the_wallet_exactly_once() {
        use axum::{
            Form, Json, Router,
            extract::State,
            routing::{get, post},
        };
        use hmac::{Hmac, Mac};
        use sha2::Sha256;
        use std::{collections::HashMap, sync::Mutex};
        type Sessions = Arc<Mutex<HashMap<String, String>>>;

        async fn customer() -> Json<Value> {
            Json(
                json!({"id":"cus_fixture","livemode":false,"metadata":{"account_id":"user-alice"}}),
            )
        }
        async fn create(
            State(form): State<Sessions>,
            Form(body): Form<HashMap<String, String>>,
        ) -> Json<Value> {
            *form.lock().unwrap() = body;
            Json(json!({"id":"cs_fixture","url":"https://checkout.stripe.com/c/fixture"}))
        }
        fn session(form: &HashMap<String, String>) -> Value {
            let metadata: serde_json::Map<String, Value> = form
                .iter()
                .filter_map(|(k, v)| {
                    let key = k.strip_prefix("metadata[")?.strip_suffix(']')?;
                    Some((key.to_owned(), json!(v)))
                })
                .collect();
            json!({"id":"cs_fixture","object":"checkout.session","livemode":false,"customer":"cus_fixture","mode":"payment","payment_status":"paid","currency":"usd","amount_total":2500,"metadata":metadata,"payment_intent":{"id":"pi_fixture","status":"succeeded","amount_received":2500,"latest_charge":{"id":"ch_fixture","paid":true,"captured":true,"amount_captured":2500,"currency":"usd"}}})
        }
        async fn retrieve(State(form): State<Sessions>) -> Json<Value> {
            Json(session(&form.lock().unwrap()))
        }

        let url = std::env::var("ZUSE_CLOUD_TEST_DATABASE_URL")
            .expect("Explicit isolated PostgreSQL required");
        let admin = sqlx::PgPool::connect(&url).await.unwrap();
        let schema = format!("wallet_{}", uuid::Uuid::new_v4().simple());
        sqlx::raw_sql(&format!("CREATE SCHEMA {schema}; SET search_path={schema}"))
            .execute(&admin)
            .await
            .unwrap();
        let scoped = format!(
            "{url}{}options=-csearch_path%3D{schema}",
            if url.contains('?') { '&' } else { '?' }
        );
        let setup = sqlx::PgPool::connect(&scoped).await.unwrap();
        sqlx::raw_sql(MIGRATION).execute(&setup).await.unwrap();

        let form: Sessions = Arc::default();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut stripe = StripeOptions::new("sk_test_fixture".into(), "whsec_fixture".into());
        stripe.api_base = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(
            axum::serve(
                listener,
                Router::new()
                    .route("/customers/cus_fixture", get(customer).post(customer))
                    .route("/checkout/sessions", post(create))
                    .route("/checkout/sessions/cs_fixture", get(retrieve))
                    .with_state(form.clone()),
            )
            .into_future(),
        );
        let wallet = Wallet::open(&scoped, stripe).await.unwrap();
        assert_eq!(wallet.spendable_micros("user-alice").await.unwrap(), 0);

        assert_eq!(
            wallet
                .checkout(
                    "user-alice",
                    "cus_fixture",
                    2500,
                    "https://api-rust.zuse.sh/v1/billing/prepaid/complete",
                )
                .await
                .unwrap(),
            "https://checkout.stripe.com/c/fixture"
        );
        // Paying is what grants credit, not opening the checkout.
        assert_eq!(wallet.spendable_micros("user-alice").await.unwrap(), 0);

        let object = session(&form.lock().unwrap());
        let body = json!({"id":"evt_fixture","type":"checkout.session.completed","livemode":false,"data":{"object":object}}).to_string();
        let timestamp = chrono::Utc::now().timestamp();
        let mut mac = Hmac::<Sha256>::new_from_slice(b"whsec_fixture").unwrap();
        mac.update(format!("{timestamp}.{body}").as_bytes());
        let signature: String = mac
            .finalize()
            .into_bytes()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        let mut headers = HeaderMap::new();
        headers.insert(
            "stripe-signature",
            format!("t={timestamp},v1={signature}").parse().unwrap(),
        );
        for _ in 0..2 {
            wallet.webhook(&headers, body.as_bytes()).await.unwrap();
        }
        wallet.maintain(100).await.unwrap();
        assert_eq!(
            wallet.spendable_micros("user-alice").await.unwrap(),
            25_000_000
        );

        // A forged delivery is refused and grants nothing.
        headers.insert(
            "stripe-signature",
            format!("t={timestamp},v1={}", "0".repeat(64))
                .parse()
                .unwrap(),
        );
        assert!(wallet.webhook(&headers, body.as_bytes()).await.is_err());
        assert_eq!(
            wallet.spendable_micros("user-alice").await.unwrap(),
            25_000_000
        );

        server.abort();
        setup.close().await;
        sqlx::raw_sql(&format!("DROP SCHEMA {schema} CASCADE"))
            .execute(&admin)
            .await
            .unwrap();
        admin.close().await;
    }
    #[test]
    fn migration_matches_the_pinned_engine_schema() {
        let (_, sql) = MIGRATION.split_once('\n').unwrap();
        assert_eq!(sql, migration_sql(Dialect::Postgres).join(";\n") + ";\n");
    }
    #[test]
    fn micros_round_trip_at_six_places() {
        assert_eq!(micros(&amount(25_000).unwrap()).unwrap(), 25_000);
        assert_eq!(micros(&amount(1).unwrap()).unwrap(), 1);
        assert_eq!(micros(&amount(5_000_000).unwrap()).unwrap(), 5_000_000);
    }
    #[test]
    fn only_wallet_money_events_reach_the_ledger() {
        let event = |kind: &str, object: Value| json!({"type":kind,"data":{"object":object}});
        let wallet = json!({"metadata":{"brokeboy_prepaid_feature":"wallet"}});
        assert!(handles(&event("checkout.session.completed", wallet)));
        assert!(!handles(&event(
            "checkout.session.completed",
            json!({"metadata":{"purpose":"zuse_prepaid_v1"}})
        )));
        assert!(handles(&event("charge.refunded", json!({}))));
        assert!(handles(&event("charge.dispute.funds_withdrawn", json!({}))));
        assert!(!handles(&event("customer.subscription.updated", json!({}))));
        assert!(!handles(&event("invoice.paid", json!({}))));
    }
}
