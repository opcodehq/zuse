//! Provider-owned billing management. Existing customers and subscriptions keep their identity.
use super::BillingControl;
mod checkout;
mod credits;
mod hosted;
mod meter;
pub use credits::{CreditDispute, CreditEvent, CreditPurchase};
pub use hosted::{HostedUsageReport, MIGRATION as HOSTED_MODELS_MIGRATION};
pub mod events;
use crate::{error::ApiError, store::Store};
use async_trait::async_trait;
pub use checkout::{CheckoutSummary, CustomerPage};
pub use events::{PaymentEvent, Subscription};
use reqwest::{Client, Url};
use serde_json::{Value, json};
use std::{collections::HashMap, sync::Arc, time::Duration};
use zeroize::Zeroizing;

#[async_trait]
pub trait PortalProvider: Send + Sync {
    fn provider_id(&self) -> &'static str;
    async fn portal(&self, owner: &str, customer: Option<&str>) -> Result<String, ApiError>;
    fn supports_checkout(&self, _offer: &str) -> bool {
        false
    }
    async fn verify_customer(&self, _owner: &str, _customer: &str) -> Result<(), ApiError> {
        Err(unavailable())
    }
    async fn create_customer(&self, _owner: &str, _key: &str) -> Result<String, ApiError> {
        Err(unavailable())
    }
    async fn customer_page(
        &self,
        _owner: &str,
        _cursor: Option<&str>,
    ) -> Result<CustomerPage, ApiError> {
        Err(unavailable())
    }
    async fn checkout(
        &self,
        _owner: &str,
        _customer: Option<&str>,
        _offer: &str,
        _success: &str,
    ) -> Result<String, ApiError> {
        Err(unavailable())
    }
    async fn checkout_summary(
        &self,
        _owner: &str,
        _id: &str,
    ) -> Result<Option<CheckoutSummary>, ApiError> {
        Err(unavailable())
    }
    fn prepaid_available(&self) -> bool {
        false
    }
    async fn prepaid_balance(
        &self,
        _owner: &str,
        _customer: Option<&str>,
    ) -> Result<zuse_protocol::cloud_billing::PrepaidBalance, ApiError> {
        Ok(zuse_protocol::cloud_billing::PrepaidBalance {
            available: false,
            credit_cents: 0,
            debit_cents: 0,
            currency: "usd".into(),
        })
    }
    async fn prepaid_checkout(
        &self,
        _owner: &str,
        _customer: &str,
        _amount: i64,
        _success: &str,
    ) -> Result<String, ApiError> {
        Err(unavailable())
    }
    async fn credit_purchase(
        &self,
        _event: &CreditEvent,
    ) -> Result<Option<CreditPurchase>, ApiError> {
        Err(unavailable())
    }
    async fn credit_adjustment(
        &self,
        _customer: &str,
        _amount: i64,
        _key: &str,
        _purchase: &str,
    ) -> Result<(), ApiError> {
        Err(unavailable())
    }
    async fn report_meter(
        &self,
        _event: &super::MeterEvent,
        _customer: Option<&str>,
    ) -> Result<(), ApiError> {
        Err(unavailable())
    }
    async fn meter_total(
        &self,
        _owner: &str,
        _customer: Option<&str>,
        _meter: &str,
        _start: i64,
        _end: i64,
    ) -> Result<i64, ApiError> {
        Err(unavailable())
    }
    fn verify_event(
        &self,
        _headers: &axum::http::HeaderMap,
        _body: &[u8],
        _now: i64,
    ) -> Result<Option<PaymentEvent>, ApiError> {
        Err(unavailable())
    }
    async fn subscription(&self, _id: &str) -> Result<Subscription, ApiError> {
        Err(unavailable())
    }
}
#[derive(Clone)]
pub struct PaymentProviders {
    default: String,
    checkout: Option<Arc<checkout::CheckoutSettings>>,
    pub(super) providers: HashMap<String, Arc<dyn PortalProvider>>,
}
impl PaymentProviders {
    pub fn default_provider_id(&self) -> &str {
        &self.default
    }
    pub fn new(default: String, providers: Vec<Arc<dyn PortalProvider>>) -> anyhow::Result<Self> {
        let mut registered = HashMap::new();
        for provider in providers {
            let id = provider.provider_id();
            anyhow::ensure!(
                registered.insert(id.to_owned(), provider).is_none(),
                "Duplicate billing provider"
            );
        }
        anyhow::ensure!(
            default == "manual" || registered.contains_key(&default),
            "Default billing provider is unavailable"
        );
        Ok(Self {
            default,
            checkout: None,
            providers: registered,
        })
    }
    pub fn from_env() -> anyhow::Result<Option<Self>> {
        let mut providers: Vec<Arc<dyn PortalProvider>> = vec![];
        if let Ok(secret) = std::env::var("STRIPE_SECRET_KEY") {
            if !secret.trim().is_empty() {
                let test = secret.starts_with("sk_test_") || secret.starts_with("rk_test_");
                let live = secret.starts_with("sk_live_") || secret.starts_with("rk_live_");
                anyhow::ensure!(test || live, "Invalid Stripe credential configuration");
                if let Ok(environment) = std::env::var("STRIPE_ENVIRONMENT") {
                    anyhow::ensure!(
                        matches!(environment.as_str(), "production" | "sandbox")
                            && ((environment == "sandbox") == test),
                        "Stripe environment/key mismatch"
                    );
                }
                let return_url = std::env::var("API_ISSUER")
                    .unwrap_or_else(|_| "https://api-rust.zuse.sh".into());
                secure_url(&return_url)
                    .map_err(|_| anyhow::anyhow!("Invalid billing return origin"))?;
                providers.push(Arc::new(StripePortal {
                    http: ProviderHttp::new("https://api.stripe.com/v1/", secret)?,
                    return_url,
                    events: events::EventConfig::from_env()?,
                    checkout: checkout::ProviderCheckout::from_env()?,
                    meter_price: std::env::var("STRIPE_CLOUD_OVERAGE_PRICE_ID")
                        .ok()
                        .filter(|v| !v.trim().is_empty()),
                    credit_price: std::env::var("STRIPE_PREPAID_CREDIT_PRICE_ID")
                        .ok()
                        .filter(|v| !v.trim().is_empty()),
                    configuration: std::env::var("STRIPE_PORTAL_CONFIGURATION_ID")
                        .ok()
                        .filter(|s| !s.is_empty()),
                }));
            }
        }
        if providers.is_empty() {
            return Ok(None);
        }
        let default = std::env::var("BILLING_DEFAULT_PROVIDER")
            .unwrap_or_else(|_| providers[0].provider_id().into());
        let mut configured = Self::new(default, providers)?;
        if std::env::var("MACHINE_LIVE_CHECKOUT_ENABLED").as_deref() == Ok("true") {
            let issuer =
                std::env::var("API_ISSUER").unwrap_or_else(|_| "https://api-rust.zuse.sh".into());
            let key = Zeroizing::new(std::env::var("RELAY_MINT_PRIVATE_JWK").map_err(|_| {
                anyhow::anyhow!("Checkout requires the original platform signing key")
            })?);
            let signer = crate::tickets::TicketSigner::from_jwk(&key, &issuer)
                .map_err(|_| anyhow::anyhow!("Invalid checkout signing configuration"))?;
            configured = configured.with_checkout(signer);
        }
        Ok(Some(configured))
    }
}
fn unavailable() -> ApiError {
    ApiError::unavailable("billing_provider_unavailable")
}
fn secure_url(value: &str) -> Result<Url, ApiError> {
    let url = Url::parse(value).map_err(|_| unavailable())?;
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(unavailable());
    }
    Ok(url)
}
const STRIPE_API_VERSION: &str = "2026-09-30.endive";
struct ProviderHttp {
    client: Client,
    origin: Url,
    secret: Zeroizing<String>,
}
impl ProviderHttp {
    fn new(origin: &str, secret: String) -> anyhow::Result<Self> {
        let origin =
            secure_url(origin).map_err(|_| anyhow::anyhow!("Invalid billing provider endpoint"))?;
        let mut headers = reqwest::header::HeaderMap::new();
        if origin.host_str() == Some("api.stripe.com") {
            headers.insert(
                "stripe-version",
                reqwest::header::HeaderValue::from_static(STRIPE_API_VERSION),
            );
        }
        Ok(Self {
            client: Client::builder()
                .default_headers(headers)
                .redirect(reqwest::redirect::Policy::none())
                .timeout(Duration::from_secs(15))
                .build()?,
            origin,
            secret: Zeroizing::new(secret),
        })
    }
    fn request(
        &self,
        method: reqwest::Method,
        path: &[&str],
    ) -> Result<reqwest::RequestBuilder, ApiError> {
        let mut url = self.origin.clone();
        url.path_segments_mut()
            .map_err(|_| unavailable())?
            .pop_if_empty()
            .extend(path.iter().copied());
        Ok(self
            .client
            .request(method, url)
            .bearer_auth(self.secret.as_str()))
    }
    async fn json(request: reqwest::RequestBuilder) -> Result<Value, ApiError> {
        let mut response = request.send().await.map_err(|_| unavailable())?;
        if !response.status().is_success() {
            return Err(unavailable());
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| unavailable())? {
            if bytes.len() + chunk.len() > 1024 * 1024 {
                return Err(unavailable());
            }
            bytes.extend_from_slice(&chunk);
        }
        serde_json::from_slice(&bytes).map_err(|_| unavailable())
    }
}
struct StripePortal {
    meter_price: Option<String>,
    credit_price: Option<String>,
    checkout: Option<checkout::ProviderCheckout>,
    events: Option<events::EventConfig>,
    http: ProviderHttp,
    return_url: String,
    configuration: Option<String>,
}
#[async_trait]
impl PortalProvider for StripePortal {
    async fn report_meter(
        &self,
        event: &super::MeterEvent,
        customer: Option<&str>,
    ) -> Result<(), ApiError> {
        self.report_usage(event, customer.ok_or_else(unavailable)?)
            .await
    }
    async fn meter_total(
        &self,
        owner: &str,
        customer: Option<&str>,
        meter: &str,
        start: i64,
        end: i64,
    ) -> Result<i64, ApiError> {
        self.usage_total(owner, customer.ok_or_else(unavailable)?, meter, start, end)
            .await
    }

    fn prepaid_available(&self) -> bool {
        self.credit_price.is_some() && self.events.is_some()
    }
    async fn prepaid_balance(
        &self,
        owner: &str,
        customer: Option<&str>,
    ) -> Result<zuse_protocol::cloud_billing::PrepaidBalance, ApiError> {
        self.credit_balance(owner, customer).await
    }
    async fn prepaid_checkout(
        &self,
        owner: &str,
        customer: &str,
        amount: i64,
        success: &str,
    ) -> Result<String, ApiError> {
        self.credit_checkout(owner, customer, amount, success).await
    }
    async fn credit_purchase(
        &self,
        event: &CreditEvent,
    ) -> Result<Option<CreditPurchase>, ApiError> {
        self.purchase(event).await
    }
    async fn credit_adjustment(
        &self,
        customer: &str,
        amount: i64,
        key: &str,
        purchase: &str,
    ) -> Result<(), ApiError> {
        ProviderHttp::json(
            self.http
                .request(
                    reqwest::Method::POST,
                    &["customers", customer, "balance_transactions"],
                )?
                .header("Idempotency-Key", key)
                .form(&[
                    ("amount", amount.to_string()),
                    ("currency", "usd".into()),
                    ("description", "Zuse prepaid invoice balance".into()),
                    ("metadata[zuse_operation]", key.into()),
                    ("metadata[checkout_id]", purchase.into()),
                ]),
        )
        .await?;
        Ok(())
    }
    fn supports_checkout(&self, offer: &str) -> bool {
        self.events.is_some()
            && self
                .checkout
                .as_ref()
                .is_some_and(|c| c.offers.contains_key(offer))
    }
    async fn verify_customer(&self, owner: &str, id: &str) -> Result<(), ApiError> {
        let value = ProviderHttp::json(
            self.http
                .request(reqwest::Method::GET, &["customers", id])?,
        )
        .await?;
        if value["id"] != id || value["deleted"] == true || value["metadata"]["account_id"] != owner
        {
            return Err(unavailable());
        }
        Ok(())
    }
    async fn create_customer(&self, owner: &str, key: &str) -> Result<String, ApiError> {
        let value = ProviderHttp::json(
            self.http
                .request(reqwest::Method::POST, &["customers"])?
                .header("Idempotency-Key", key)
                .form(&[("metadata[account_id]", owner)]),
        )
        .await?;
        if value["metadata"]["account_id"] != owner {
            return Err(unavailable());
        }
        Ok(checkout::required(&value, "id")?.into())
    }
    async fn customer_page(
        &self,
        owner: &str,
        cursor: Option<&str>,
    ) -> Result<CustomerPage, ApiError> {
        let mut request = self
            .http
            .request(reqwest::Method::GET, &["customers"])?
            .query(&[("limit", "100")]);
        if let Some(cursor) = cursor {
            request = request.query(&[("starting_after", cursor)]);
        }
        let value = ProviderHttp::json(request).await?;
        let data = value["data"].as_array().ok_or_else(unavailable)?;
        let mut matches = vec![];
        for customer in data {
            if customer["metadata"]["account_id"] == owner && customer["deleted"] != true {
                matches.push(checkout::required(customer, "id")?.into());
            }
        }
        let more = value["has_more"].as_bool().ok_or_else(unavailable)?;
        let next_cursor = if more {
            Some(checkout::required(data.last().ok_or_else(unavailable)?, "id")?.into())
        } else {
            None
        };
        Ok(CustomerPage {
            matches,
            next_cursor,
        })
    }
    async fn checkout(
        &self,
        owner: &str,
        customer: Option<&str>,
        offer: &str,
        success: &str,
    ) -> Result<String, ApiError> {
        let config = self.checkout.as_ref().ok_or_else(unavailable)?;
        let customer = customer.ok_or_else(unavailable)?;
        self.verify_customer(owner, customer).await?;
        let price = config.offers.get(offer).ok_or_else(unavailable)?;
        let success = success.replace("{CHECKOUT_ID}", "{CHECKOUT_SESSION_ID}");
        let mut params = vec![
            ("mode", "subscription"),
            ("allow_promotion_codes", "true"),
            ("customer", customer),
            ("client_reference_id", owner),
            ("success_url", success.as_str()),
            ("line_items[0][price]", price.as_str()),
            ("line_items[0][quantity]", "1"),
            ("metadata[account_id]", owner),
            ("metadata[offer_id]", offer),
            ("subscription_data[metadata][account_id]", owner),
            ("subscription_data[metadata][offer_id]", offer),
            (
                "automatic_tax[enabled]",
                if config.automatic_tax {
                    "true"
                } else {
                    "false"
                },
            ),
            ("billing_address_collection", "required"),
            ("tax_id_collection[enabled]", "true"),
            ("customer_update[address]", "auto"),
            ("customer_update[name]", "auto"),
        ];
        if offer == checkout::CLOUD_OFFER {
            if let Some(overage) = &config.overage {
                params.push(("line_items[1][price]", overage.as_str()));
            }
        }
        let result = ProviderHttp::json(
            self.http
                .request(reqwest::Method::POST, &["checkout", "sessions"])?
                .form(&params),
        )
        .await?;
        let url = checkout::required(&result, "url")?;
        secure_url(url)?;
        Ok(url.into())
    }
    async fn checkout_summary(
        &self,
        owner: &str,
        id: &str,
    ) -> Result<Option<CheckoutSummary>, ApiError> {
        let value = ProviderHttp::json(
            self.http
                .request(reqwest::Method::GET, &["checkout", "sessions", id])?,
        )
        .await?;
        if value["id"] != id || value["client_reference_id"] != owner {
            return Ok(None);
        }
        checkout::CheckoutSummary::stripe(&value).map(Some)
    }
    fn provider_id(&self) -> &'static str {
        "stripe"
    }
    fn verify_event(
        &self,
        headers: &axum::http::HeaderMap,
        body: &[u8],
        now: i64,
    ) -> Result<Option<PaymentEvent>, ApiError> {
        self.events
            .as_ref()
            .ok_or_else(unavailable)?
            .stripe_event(headers, body, now)
    }
    async fn subscription(&self, id: &str) -> Result<Subscription, ApiError> {
        let value = ProviderHttp::json(
            self.http
                .request(reqwest::Method::GET, &["subscriptions", id])?
                .query(&[("expand[]", "customer")]),
        )
        .await?;
        self.events
            .as_ref()
            .ok_or_else(unavailable)?
            .stripe_subscription(id, &value)
    }
    async fn portal(&self, owner: &str, customer: Option<&str>) -> Result<String, ApiError> {
        let id = customer.filter(|s| !s.is_empty()).ok_or_else(unavailable)?;
        let customer = ProviderHttp::json(
            self.http
                .request(reqwest::Method::GET, &["customers", id])?,
        )
        .await?;
        if customer["id"] != id
            || customer["deleted"] == true
            || customer["metadata"]["account_id"] != owner
        {
            return Err(unavailable());
        }
        let mut params = vec![("customer", id), ("return_url", self.return_url.as_str())];
        if let Some(config) = &self.configuration {
            params.push(("configuration", config));
        }
        let session = ProviderHttp::json(
            self.http
                .request(reqwest::Method::POST, &["billing_portal", "sessions"])?
                .form(&params),
        )
        .await?;
        if session["customer"] != id {
            return Err(unavailable());
        }
        let url = session["url"].as_str().ok_or_else(unavailable)?;
        secure_url(url)?;
        Ok(url.into())
    }
}
impl BillingControl {
    pub fn with_payment_providers(mut self, providers: PaymentProviders) -> Self {
        self.payments = Some(Arc::new(providers));
        self
    }
    pub fn with_wallet(mut self, wallet: crate::billing::Wallet) -> Self {
        self.wallet = Some(Arc::new(wallet));
        self
    }
    async fn portal(&self, owner: &str) -> Result<String, ApiError> {
        let providers = self.payments.as_ref().ok_or_else(unavailable)?;
        let mut tx = self.owned_transaction(owner).await?;
        let ids:Vec<String>=sqlx::query_scalar("SELECT DISTINCT provider FROM api_entitlements WHERE account_id=$1 AND provider_subscription_id IS NOT NULL AND status<>'ended'").bind(owner).fetch_all(&mut *tx).await?;
        if ids.len() > 1 {
            return Err(ApiError::conflict("billing_provider_ambiguous"));
        }
        let id = ids
            .first()
            .map(String::as_str)
            .unwrap_or(&providers.default);
        let provider = providers.providers.get(id).ok_or_else(unavailable)?;
        let customer: Option<String> = if id == "stripe" {
            sqlx::query_scalar("SELECT customer_id FROM api_stripe_customers WHERE account_id=$1")
                .bind(owner)
                .fetch_optional(&mut *tx)
                .await?
                .flatten()
        } else {
            None
        };
        let url = provider.portal(owner, customer.as_deref()).await?;
        secure_url(&url)?;
        tx.commit().await?;
        Ok(url)
    }
}
impl Store {
    pub async fn billing_portal(&self, owner: &str) -> Result<String, ApiError> {
        self.billing
            .as_ref()
            .ok_or_else(|| ApiError::unavailable("cloud_billing_writes_disabled"))?
            .portal(owner)
            .await
    }
}
#[cfg(test)]
mod tests;

pub(super) fn stripe_meter_command(
    event: &super::MeterEvent,
    customer: &str,
    now: i64,
) -> Result<(String, String), ApiError> {
    let command = meter::StripeCommand::new(event, customer, now)?;
    Ok((
        command.key().into(),
        serde_json::to_string(&command)
            .map_err(|_| ApiError::conflict("billing_meter_reconciliation_required"))?,
    ))
}
