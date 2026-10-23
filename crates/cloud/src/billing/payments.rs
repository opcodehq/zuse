//! Provider-owned billing management. Existing customers and subscriptions keep their identity.
use super::BillingControl;
use crate::{error::ApiError, store::Store};
use async_trait::async_trait;
use reqwest::{Client, Url};
use serde_json::{Value, json};
use std::{collections::HashMap, sync::Arc, time::Duration};
use zeroize::Zeroizing;

#[async_trait]
pub trait PortalProvider: Send + Sync {
    fn provider_id(&self) -> &'static str;
    async fn portal(&self, owner: &str, customer: Option<&str>) -> Result<String, ApiError>;
}
#[derive(Clone)]
pub struct PaymentProviders {
    default: String,
    providers: HashMap<String, Arc<dyn PortalProvider>>,
}
impl PaymentProviders {
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
                    configuration: std::env::var("STRIPE_PORTAL_CONFIGURATION_ID")
                        .ok()
                        .filter(|s| !s.is_empty()),
                }));
            }
        }
        if let Ok(secret) = std::env::var("POLAR_ACCESS_TOKEN") {
            if !secret.trim().is_empty() {
                let environment =
                    std::env::var("POLAR_ENVIRONMENT").unwrap_or_else(|_| "production".into());
                let origin = match environment.as_str() {
                    "production" => "https://api.polar.sh/v1/",
                    "sandbox" => "https://sandbox-api.polar.sh/v1/",
                    _ => anyhow::bail!("Invalid Polar environment"),
                };
                providers.push(Arc::new(PolarPortal {
                    http: ProviderHttp::new(origin, secret)?,
                }));
            }
        }
        if providers.is_empty() {
            return Ok(None);
        }
        let default = std::env::var("BILLING_DEFAULT_PROVIDER")
            .unwrap_or_else(|_| providers[0].provider_id().into());
        Ok(Some(Self::new(default, providers)?))
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
struct ProviderHttp {
    client: Client,
    origin: Url,
    secret: Zeroizing<String>,
}
impl ProviderHttp {
    fn new(origin: &str, secret: String) -> anyhow::Result<Self> {
        Ok(Self {
            client: Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(Duration::from_secs(15))
                .build()?,
            origin: secure_url(origin)
                .map_err(|_| anyhow::anyhow!("Invalid billing provider endpoint"))?,
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
    http: ProviderHttp,
    return_url: String,
    configuration: Option<String>,
}
#[async_trait]
impl PortalProvider for StripePortal {
    fn provider_id(&self) -> &'static str {
        "stripe"
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
struct PolarPortal {
    http: ProviderHttp,
}
#[async_trait]
impl PortalProvider for PolarPortal {
    fn provider_id(&self) -> &'static str {
        "polar"
    }
    async fn portal(&self, owner: &str, _customer: Option<&str>) -> Result<String, ApiError> {
        let session = ProviderHttp::json(
            self.http
                .request(reqwest::Method::POST, &["customer-sessions", ""])?
                .json(&json!({"external_customer_id":owner})),
        )
        .await?;
        let url = session["customer_portal_url"]
            .as_str()
            .ok_or_else(unavailable)?;
        secure_url(url)?;
        Ok(url.into())
    }
}
impl BillingControl {
    pub fn with_payment_providers(mut self, providers: PaymentProviders) -> Self {
        self.payments = Some(Arc::new(providers));
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
